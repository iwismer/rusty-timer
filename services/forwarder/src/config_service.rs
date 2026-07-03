//! Config persistence for the forwarder.
//!
//! Owns the on-disk TOML config file: serialized read-modify-write access
//! (via [`ConfigState`]), atomic writes, JSON serialization shared by the
//! local status HTTP surface and the P2P remote-config verbs, and the
//! per-section update logic behind `POST /api/v1/config/{section}`.

use std::io::Write as _;
use std::net::SocketAddrV4;
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::status_http::{
    apply_control_action_from_config, bad_request_error, require_object_payload,
};
use crate::status_store::{SubsystemStatus, mark_restart_needed_and_emit};

/// Holds the config file path and a write lock for read-modify-write operations.
pub struct ConfigState {
    pub path: std::path::PathBuf,
    pub(crate) write_lock: Mutex<()>,
}

impl ConfigState {
    pub fn new(path: std::path::PathBuf) -> Self {
        ConfigState {
            path,
            write_lock: Mutex::new(()),
        }
    }
}

fn write_atomic(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let original_permissions = std::fs::metadata(path).map(|m| m.permissions()).ok();

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path has no parent: {}", path.display()),
        )
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", path.display()),
        )
    })?;

    let file_name = file_name.to_string_lossy();
    let pid = std::process::id();

    for attempt in 0..=16 {
        let tmp_name = format!(".{}.tmp.{}.{}", file_name, pid, attempt);
        let tmp_path = parent.join(tmp_name);
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)
        {
            Ok(mut temp_file) => {
                let result = (|| -> std::io::Result<()> {
                    temp_file.write_all(content.as_bytes())?;
                    temp_file.sync_all()?;

                    if let Some(perms) = &original_permissions {
                        std::fs::set_permissions(&tmp_path, perms.clone())?;
                    }

                    std::fs::rename(&tmp_path, path)?;
                    if let Ok(parent_dir) = std::fs::File::open(parent) {
                        let _ = parent_dir.sync_all();
                    }
                    Ok(())
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(&tmp_path);
                }
                return result;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("failed to allocate temp path for {}", path.display()),
    ))
}

/// Read the config TOML file as a JSON value plus the current `restart_needed`
/// state, returning a plain error message on failure.
///
/// Shared core for both the HTTP `GET /api/v1/config` endpoint and the P2P
/// remote-config get verb, so the two always agree on the serialized shape.
async fn read_config_value(
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
) -> Result<(serde_json::Value, bool), String> {
    let _lock = config_state.write_lock.lock().await;

    let toml_str =
        std::fs::read_to_string(&config_state.path).map_err(|e| format!("File read error: {e}"))?;

    let raw: crate::config::RawConfig =
        toml::from_str(&toml_str).map_err(|e| format!("TOML parse error: {e}"))?;

    let json = serde_json::to_value(&raw).map_err(|e| format!("JSON serialize error: {e}"))?;

    let restart_needed = subsystem.lock().await.restart_needed();
    Ok((json, restart_needed))
}

/// Read the config TOML file as JSON.
///
/// Returns `(config_json, restart_needed)` on success.
pub async fn read_config_json(
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
) -> Result<(serde_json::Value, bool), (u16, String)> {
    read_config_value(config_state, subsystem)
        .await
        .map_err(|e| {
            (
                500u16,
                serde_json::json!({"ok": false, "error": e}).to_string(),
            )
        })
}

/// Serialize the current config to a JSON string (identical to the body
/// `GET /api/v1/config` returns) plus the current `restart_needed` state.
///
/// Used by the P2P remote-config get verb so the receiver UI round-trips the
/// same document whether it reads config over HTTP or P2P.
pub async fn config_json_string(
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
) -> Result<(String, bool), String> {
    let (value, restart_needed) = read_config_value(config_state, subsystem).await?;
    let json = serde_json::to_string(&value).map_err(|e| format!("JSON serialize error: {e}"))?;
    Ok((json, restart_needed))
}

/// Sections a P2P peer may never change: identity/credentials (`auth`),
/// transport trust (`p2p`), and the gates themselves (`control`).
///
/// Boundary rationale: `[auth]`/`[p2p]`/`[control]` changes are trust/identity
/// escalation — permanent and self-granting; `[journal]`, `[[readers]]`, and
/// display fields are the operational surface remote management exists to serve
/// — a hostile allow-listed receiver can disrupt operations through them but
/// cannot expand its own access. `[status_http]`, `[update]`, and `[ups]` were
/// considered and left writable as operational surface; `status_http.bind` is
/// the borderline case (a receiver could rebind the local admin HTTP), but it
/// grants the receiver itself no access.
const REMOTE_PROTECTED_SECTIONS: &[&str] = &["auth", "p2p", "control"];

/// Persist a full config document received from P2P remote config, rejecting
/// any attempted change to privileged config sections before writing.
///
/// The document is parsed into the same [`crate::config::RawConfig`] the get
/// path serializes, re-serialized to TOML, and validated by running the
/// canonical loader before anything is written — so a document that would fail
/// to load on restart is rejected without corrupting the on-disk file. Reuses
/// the same atomic writer and `restart_needed` signal as the per-section HTTP
/// writers. Returns a plain error message on failure.
///
/// There is intentionally no unrestricted full-document writer: local
/// (trusted) config edits go through the per-section HTTP handlers
/// ([`apply_section_update`] / [`update_config_file`]), so every full-document
/// write path enforces [`REMOTE_PROTECTED_SECTIONS`].
pub async fn write_config_json_restricted(
    config_json: &str,
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
) -> Result<(), String> {
    let incoming: crate::config::RawConfig =
        serde_json::from_str(config_json).map_err(|e| format!("invalid config JSON: {e}"))?;

    let _lock = config_state.write_lock.lock().await;

    let current_toml =
        std::fs::read_to_string(&config_state.path).map_err(|e| format!("File read error: {e}"))?;
    let current: crate::config::RawConfig =
        toml::from_str(&current_toml).map_err(|e| format!("TOML parse error: {e}"))?;

    let incoming_value =
        serde_json::to_value(&incoming).map_err(|e| format!("JSON serialize error: {e}"))?;
    let current_value =
        serde_json::to_value(&current).map_err(|e| format!("JSON serialize error: {e}"))?;

    for section in REMOTE_PROTECTED_SECTIONS {
        if normalize_section(incoming_value.get(section))
            != normalize_section(current_value.get(section))
        {
            return Err(format!(
                "remote config may not modify the protected [{section}] section"
            ));
        }
    }

    write_config_json_locked(incoming, config_state, subsystem, ui_tx).await
}

/// Shared locked write body. Caller must hold `config_state.write_lock`.
///
/// Takes the already-parsed [`crate::config::RawConfig`] (the same value the
/// protected-section comparison ran against) so "what was compared is what is
/// written" holds structurally rather than by re-parsing the same string.
async fn write_config_json_locked(
    raw: crate::config::RawConfig,
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
) -> Result<(), String> {
    let new_toml =
        toml::to_string_pretty(&raw).map_err(|e| format!("TOML serialize error: {e}"))?;

    // Validate via the canonical loader so we never persist a config that would
    // fail to load on the next restart.
    crate::config::load_config_from_str(&new_toml, &config_state.path)
        .map_err(|e| format!("config validation failed: {e}"))?;

    write_atomic(&config_state.path, &new_toml).map_err(|e| format!("File write error: {e}"))?;

    mark_restart_needed_and_emit(subsystem, ui_tx).await;
    Ok(())
}

/// A missing section, `null`, and an all-null object are equivalent RawConfig
/// states.
///
/// Normalization is single-level by design: every protected raw section is a
/// flat struct of scalars/`Vec<String>` today. If a nested struct is ever
/// added to a protected section, an all-null nested object will not be
/// flattened — the failure mode is a false *reject* (fail-closed), and the
/// populated-section round-trip test in `p2p/remote_config.rs` will catch
/// serialization drift.
fn normalize_section(v: Option<&serde_json::Value>) -> serde_json::Value {
    match v {
        None | Some(serde_json::Value::Null) => serde_json::Value::Null,
        Some(serde_json::Value::Object(map)) if map.values().all(serde_json::Value::is_null) => {
            serde_json::Value::Null
        }
        Some(other) => other.clone(),
    }
}

/// Read the TOML config file, apply a mutation, and write it back.
///
/// Returns Ok(()) on success or Err((status_code, json_error_body)) on failure.
async fn update_config_file(
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
    mutate: impl FnOnce(&mut crate::config::RawConfig) -> Result<(), String>,
) -> Result<(), (u16, String)> {
    let _lock = config_state.write_lock.lock().await;

    let toml_str = std::fs::read_to_string(&config_state.path).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("File read error: {}", e)})
                .to_string(),
        )
    })?;

    let mut raw: crate::config::RawConfig = toml::from_str(&toml_str).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("TOML parse error: {}", e)})
                .to_string(),
        )
    })?;

    mutate(&mut raw).map_err(|e| {
        (
            400u16,
            serde_json::json!({"ok": false, "error": e}).to_string(),
        )
    })?;

    let new_toml = toml::to_string_pretty(&raw).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("TOML serialize error: {}", e)})
                .to_string(),
        )
    })?;

    crate::config::load_config_from_str(&new_toml, &config_state.path).map_err(|e| {
        (
            400u16,
            serde_json::json!({"ok": false, "error": format!("config validation failed: {e}")})
                .to_string(),
        )
    })?;

    write_atomic(&config_state.path, &new_toml).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("File write error: {}", e)})
                .to_string(),
        )
    })?;

    mark_restart_needed_and_emit(subsystem, ui_tx).await;
    Ok(())
}

/// Apply a config section update by name.
///
/// Dispatches to the right mutation logic based on `section`, validates the
/// payload, and calls `update_config_file` to persist the change.
///
/// Recognised sections: `"general"`, `"auth"`, `"journal"`, `"status_http"`,
/// `"control"`, `"update"`, `"p2p"`, `"ups"`, `"readers"`, and `"screen"`.
/// Screen config changes require a restart to apply.
pub async fn apply_section_update(
    section: &str,
    payload: &serde_json::Value,
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
    logger: Option<&rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
) -> Result<(), (u16, String)> {
    require_object_payload(payload)?;

    match section {
        "general" => {
            let display_name = optional_string_field(payload, "display_name")?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.display_name = display_name;
                Ok(())
            })
            .await
        }
        "auth" => {
            let token_file_opt = optional_string_field(payload, "token_file")?;
            let token_file = require_non_empty_trimmed("token_file", token_file_opt)
                .map_err(bad_request_error)?;
            validate_token_file(&token_file).map_err(bad_request_error)?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.auth = Some(crate::config::RawAuthConfig {
                    token_file: Some(token_file),
                });
                Ok(())
            })
            .await
        }
        "journal" => {
            let sqlite_path = optional_string_field(payload, "sqlite_path")?;
            let prune_watermark_pct = optional_u8_field(payload, "prune_watermark_pct")?;
            let min_retention = optional_string_field(payload, "min_retention")?;
            let max_retention = optional_string_field(payload, "max_retention")?;
            let emergency_free_disk_bytes =
                optional_u64_field(payload, "emergency_free_disk_bytes")?;
            let emergency_max_rows = optional_u64_field(payload, "emergency_max_rows")?
                .map(|value| {
                    i64::try_from(value)
                        .map_err(|_| bad_request_error("emergency_max_rows must be <= i64::MAX"))
                })
                .transpose()?;
            if let Some(pct) = prune_watermark_pct
                && pct > 100
            {
                return Err(bad_request_error(
                    "prune_watermark_pct must be between 0 and 100",
                ));
            }
            crate::config::validate_retention_settings(
                min_retention.as_deref(),
                max_retention.as_deref(),
                emergency_max_rows,
            )
            .map_err(bad_request_error)?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.journal = Some(crate::config::RawJournalConfig {
                    sqlite_path,
                    prune_watermark_pct,
                    min_retention,
                    max_retention,
                    emergency_free_disk_bytes,
                    emergency_max_rows,
                });
                Ok(())
            })
            .await
        }
        "status_http" => {
            let bind = optional_string_field(payload, "bind")?.and_then(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            });
            if let Some(ref bind_addr) = bind {
                validate_status_bind(bind_addr).map_err(bad_request_error)?;
            }
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.status_http = Some(crate::config::RawStatusHttpConfig { bind });
                Ok(())
            })
            .await
        }
        "control" => {
            let allow_power_actions = optional_bool_field(payload, "allow_power_actions")?;
            let action = optional_string_field(payload, "action")?;
            if let Some(action) = action {
                return apply_control_action_from_config(&action, config_state, logger).await;
            }
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                // Preserve existing P2P control settings; this handler only
                // mutates allow_power_actions.
                let allow_remote_config = raw.control.as_ref().and_then(|c| c.allow_remote_config);
                let allow_reader_control =
                    raw.control.as_ref().and_then(|c| c.allow_reader_control);
                raw.control = Some(crate::config::RawControlConfig {
                    allow_power_actions,
                    allow_remote_config,
                    allow_reader_control,
                });
                Ok(())
            })
            .await
        }
        "p2p" => {
            let enabled = optional_bool_field(payload, "enabled")?;
            let server_url = optional_string_field(payload, "server_url")?.and_then(|url| {
                let trimmed = url.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            });
            let server_token_file =
                optional_string_field(payload, "server_token_file")?.and_then(|path| {
                    let trimmed = path.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_owned())
                });
            if let Some(ref url) = server_url {
                validate_server_url(url).map_err(bad_request_error)?;
            }
            if let Some(ref token_file) = server_token_file {
                validate_token_file(token_file).map_err(bad_request_error)?;
            }
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                let previous = raw.p2p.take();
                raw.p2p = Some(crate::config::RawP2pConfig {
                    enabled,
                    secret_key_path: previous
                        .as_ref()
                        .and_then(|cfg| cfg.secret_key_path.clone()),
                    secret_key_seed_hex: previous
                        .as_ref()
                        .and_then(|cfg| cfg.secret_key_seed_hex.clone()),
                    bind_addr_v4: previous.as_ref().and_then(|cfg| cfg.bind_addr_v4.clone()),
                    relay_disabled: previous.as_ref().and_then(|cfg| cfg.relay_disabled),
                    discovery_disabled: previous.as_ref().and_then(|cfg| cfg.discovery_disabled),
                    max_concurrent_bidi_streams: previous
                        .as_ref()
                        .and_then(|cfg| cfg.max_concurrent_bidi_streams),
                    static_allowed_receivers: previous
                        .as_ref()
                        .and_then(|cfg| cfg.static_allowed_receivers.clone()),
                    allowlist_cache_path: previous
                        .as_ref()
                        .and_then(|cfg| cfg.allowlist_cache_path.clone()),
                    server_url,
                    server_token_file,
                    device_token_file: previous
                        .as_ref()
                        .and_then(|cfg| cfg.device_token_file.clone()),
                    allowlist_poll_interval_secs: previous
                        .as_ref()
                        .and_then(|cfg| cfg.allowlist_poll_interval_secs),
                    allowlist_request_timeout_secs: previous
                        .as_ref()
                        .and_then(|cfg| cfg.allowlist_request_timeout_secs),
                });
                Ok(())
            })
            .await
        }
        "update" => {
            let mode_str = optional_string_field(payload, "mode")?;
            let parsed_mode = match mode_str.as_ref() {
                Some(m) => serde_json::from_value::<rt_updater::UpdateMode>(
                    serde_json::Value::String(m.clone()),
                )
                .map_err(|_| {
                    (
                        400u16,
                        serde_json::json!({"ok": false, "error": format!(
                            "mode must be 'disabled', 'check-only', or 'check-and-download', got '{}'", m
                        )})
                        .to_string(),
                    )
                })?,
                None => rt_updater::UpdateMode::default(),
            };
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.update = Some(crate::config::RawUpdateConfig { mode: mode_str });
                Ok(())
            })
            .await?;
            subsystem.lock().await.update_mode = parsed_mode;
            Ok(())
        }
        "ups" => {
            let enabled = optional_bool_field(payload, "enabled")?;
            let daemon_addr = optional_string_field(payload, "daemon_addr")?.and_then(|addr| {
                let trimmed = addr.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            });
            let poll_interval_secs = optional_u64_field(payload, "poll_interval_secs")?;
            let upstream_heartbeat_secs = optional_u64_field(payload, "upstream_heartbeat_secs")?;

            if let Some(interval) = poll_interval_secs
                && !(1..=60).contains(&interval)
            {
                return Err(bad_request_error(
                    "poll_interval_secs must be between 1 and 60",
                ));
            }
            if let Some(heartbeat) = upstream_heartbeat_secs
                && !(10..=300).contains(&heartbeat)
            {
                return Err(bad_request_error(
                    "upstream_heartbeat_secs must be between 10 and 300",
                ));
            }
            if let Some(ref addr) = daemon_addr
                && addr.parse::<std::net::SocketAddr>().is_err()
            {
                let parts: Vec<&str> = addr.rsplitn(2, ':').collect();
                if parts.len() != 2 || parts[0].parse::<u16>().is_err() {
                    return Err(bad_request_error(format!(
                        "daemon_addr must be a valid host:port, got '{}'",
                        addr
                    )));
                }
            }

            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.ups = Some(crate::config::RawUpsConfig {
                    enabled,
                    daemon_addr,
                    poll_interval_secs,
                    upstream_heartbeat_secs,
                });
                Ok(())
            })
            .await
        }
        "readers" => {
            let readers_val = payload.get("readers").ok_or_else(|| {
                (
                    400u16,
                    serde_json::json!({"ok": false, "error": "readers field is required"})
                        .to_string(),
                )
            })?;
            let readers_arr = readers_val.as_array().ok_or_else(|| {
                (
                    400u16,
                    serde_json::json!({"ok": false, "error": "readers must be an array"})
                        .to_string(),
                )
            })?;

            if readers_arr.is_empty() {
                return Err((
                    400u16,
                    "{\"ok\":false,\"error\":\"at least one reader is required\"}".to_owned(),
                ));
            }

            let mut raw_readers = Vec::with_capacity(readers_arr.len());
            for (i, entry) in readers_arr.iter().enumerate() {
                let target = optional_string_field(entry, "target")?;

                let target_str = match &target {
                    Some(t) => t,
                    None => {
                        return Err((
                            400u16,
                            serde_json::json!({"ok": false, "error": format!("readers[{}].target is required", i)}).to_string(),
                        ));
                    }
                };

                if let Err(e) = crate::discovery::expand_target(target_str) {
                    return Err((
                        400u16,
                        serde_json::json!({"ok": false, "error": format!("readers[{}].target invalid: {}", i, e)}).to_string(),
                    ));
                }

                let enabled = optional_bool_field(entry, "enabled")?;
                let local_fallback_port = optional_u16_field(entry, "local_fallback_port")?;

                raw_readers.push(crate::config::RawReaderConfig {
                    target,
                    enabled,
                    local_fallback_port,
                });
            }

            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.readers = Some(raw_readers);
                Ok(())
            })
            .await
        }
        #[cfg(any(feature = "eink", feature = "lcd"))]
        "screen" => {
            let parsed = serde_json::from_value::<rt_screen::state::ScreenConfig>(payload.clone())
                .map_err(|e| bad_request_error(e.to_string()))?;
            // Validate before persisting so the endpoint can't accept values the
            // loader would later reject (which would make the forwarder fail to
            // boot on the very restart this change requests).
            crate::config::validate_screen_config(&parsed)
                .map_err(|e| bad_request_error(e.to_string()))?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.screen = Some(parsed);
                Ok(())
            })
            .await
        }
        _ => Err((
            400u16,
            serde_json::json!({"ok": false, "error": format!("unknown section: {}", section)})
                .to_string(),
        )),
    }
}

fn optional_string_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<String>, (u16, String)> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(bad_request_error(format!(
            "{} must be a string or null",
            field
        ))),
    }
}

fn optional_bool_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<bool>, (u16, String)> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad_request_error(format!(
            "{} must be a boolean or null",
            field
        ))),
    }
}

fn optional_u64_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u64>, (u16, String)> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => {
            let raw = value.as_u64().ok_or_else(|| {
                bad_request_error(format!("{} must be a non-negative integer or null", field))
            })?;
            Ok(Some(raw))
        }
    }
}

fn optional_u16_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u16>, (u16, String)> {
    let raw = optional_u64_field(payload, field)?;
    raw.map(|value| {
        u16::try_from(value)
            .map_err(|_| bad_request_error(format!("{} must be <= {}", field, u16::MAX)))
    })
    .transpose()
}

fn optional_u8_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u8>, (u16, String)> {
    let raw = optional_u64_field(payload, field)?;
    raw.map(|value| {
        u8::try_from(value)
            .map_err(|_| bad_request_error(format!("{} must be <= {}", field, u8::MAX)))
    })
    .transpose()
}

fn require_non_empty_trimmed(field: &str, value: Option<String>) -> Result<String, String> {
    let raw = value.ok_or_else(|| format!("{} is required", field))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{} must not be empty", field));
    }
    Ok(trimmed.to_owned())
}

fn validate_token_file(token_file: &str) -> Result<(), String> {
    if token_file.contains('\n') || token_file.contains('\r') {
        return Err("token_file must be a single-line path".to_owned());
    }
    Ok(())
}

fn validate_status_bind(bind: &str) -> Result<(), String> {
    bind.parse::<SocketAddrV4>()
        .map(|_| ())
        .map_err(|_| "bind must be a valid IPv4 address with port (e.g. 127.0.0.1:8080)".to_owned())
}

fn validate_server_url(url: &str) -> Result<(), String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err("server_url must start with http:// or https://".to_owned())
    }
}
