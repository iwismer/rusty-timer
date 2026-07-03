//! Production remote-config handler for the P2P control plane.
//!
//! [`ForwarderRemoteConfigHandler`] serves the `ConfigGet` / `ConfigSet` /
//! `Restart` verbs over the control stream, reusing the exact same config
//! serialization, persistence, and restart mechanisms as the local status
//! HTTP surface (see [`crate::status_http`]):
//!
//! - get -> [`crate::status_http::config_json_string`] (same body as
//!   `GET /api/v1/config`),
//! - set -> [`crate::status_http::write_config_json_restricted`] (rejects
//!   privileged section changes, parses the full config document, validates via
//!   the canonical loader, writes the TOML file atomically, and marks a restart
//!   as needed),
//! - restart -> the same `restart_signal` [`Notify`] the HTTP restart endpoint
//!   triggers.
//!
//! The whole feature is gated by a single forwarder flag,
//! `control.allow_remote_config`. When it is `false` the capability is not
//! advertised (see [`crate::p2p::control`]) *and* every verb is rejected here
//! as defense in depth: set/restart return `ok`/`accepted = false` with the
//! error "remote config disabled", and get returns an empty `config_json` as
//! the explicit "disabled" signal (the get response shape has no error field).

use std::sync::Arc;

use rt_p2p_protocol::{
    ConfigGetRequest, ConfigGetResponse, ConfigSetRequest, ConfigSetResponse, RestartRequest,
    RestartResponse,
};
use tokio::sync::{Mutex, Notify, broadcast};

use crate::status_http::{
    ConfigState, SubsystemStatus, config_json_string, write_config_json_restricted,
};
use crate::ui_events::ForwarderUiEvent;

use super::control::{
    ConfigGetFuture, ConfigSetFuture, REMOTE_CONFIG_DISABLED, RemoteConfigHandler, RestartFuture,
};

/// Remote-config handler backed by the forwarder's on-disk TOML config and the
/// running status server's restart signal.
pub struct ForwarderRemoteConfigHandler {
    allow_remote_config: bool,
    config_state: Arc<ConfigState>,
    subsystem: Arc<Mutex<SubsystemStatus>>,
    ui_tx: broadcast::Sender<ForwarderUiEvent>,
    restart_signal: Arc<Notify>,
}

impl std::fmt::Debug for ForwarderRemoteConfigHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForwarderRemoteConfigHandler")
            .field("allow_remote_config", &self.allow_remote_config)
            .field("config_path", &self.config_state.path)
            .finish_non_exhaustive()
    }
}

impl ForwarderRemoteConfigHandler {
    /// Builds a handler. `allow_remote_config` mirrors
    /// `control.allow_remote_config` and gates both capability advertisement
    /// and every verb.
    #[must_use]
    pub fn new(
        allow_remote_config: bool,
        config_state: Arc<ConfigState>,
        subsystem: Arc<Mutex<SubsystemStatus>>,
        ui_tx: broadcast::Sender<ForwarderUiEvent>,
        restart_signal: Arc<Notify>,
    ) -> Self {
        Self {
            allow_remote_config,
            config_state,
            subsystem,
            ui_tx,
            restart_signal,
        }
    }
}

impl RemoteConfigHandler for ForwarderRemoteConfigHandler {
    fn allow_remote_config(&self) -> bool {
        self.allow_remote_config
    }

    fn get_config(&self, request: ConfigGetRequest) -> ConfigGetFuture<'_> {
        Box::pin(async move {
            if !self.allow_remote_config {
                // Disabled: empty config_json is the explicit "unavailable"
                // signal in a response shape that has no error field.
                return ConfigGetResponse {
                    request_id: request.request_id,
                    config_json: String::new(),
                    restart_needed: false,
                };
            }
            match config_json_string(&self.config_state, &self.subsystem).await {
                Ok((config_json, restart_needed)) => ConfigGetResponse {
                    request_id: request.request_id,
                    config_json,
                    restart_needed,
                },
                Err(error) => {
                    tracing::warn!(%error, "p2p: remote config get failed");
                    // No error field on get; surface failure as empty config so
                    // the receiver does not treat a read error as valid config.
                    ConfigGetResponse {
                        request_id: request.request_id,
                        config_json: String::new(),
                        restart_needed: false,
                    }
                }
            }
        })
    }

    fn set_config(&self, request: ConfigSetRequest) -> ConfigSetFuture<'_> {
        Box::pin(async move {
            if !self.allow_remote_config {
                return ConfigSetResponse {
                    request_id: request.request_id,
                    ok: false,
                    restart_needed: false,
                    error: REMOTE_CONFIG_DISABLED.to_owned(),
                };
            }
            match write_config_json_restricted(
                &request.config_json,
                &self.config_state,
                &self.subsystem,
                &self.ui_tx,
            )
            .await
            {
                Ok(()) => ConfigSetResponse {
                    request_id: request.request_id,
                    ok: true,
                    restart_needed: true,
                    error: String::new(),
                },
                Err(error) => {
                    tracing::warn!(%error, "p2p: remote config set failed");
                    let restart_needed = self.subsystem.lock().await.restart_needed();
                    ConfigSetResponse {
                        request_id: request.request_id,
                        ok: false,
                        restart_needed,
                        error,
                    }
                }
            }
        })
    }

    fn restart(&self, request: RestartRequest) -> RestartFuture<'_> {
        Box::pin(async move {
            if !self.allow_remote_config {
                return RestartResponse {
                    request_id: request.request_id,
                    accepted: false,
                    error: REMOTE_CONFIG_DISABLED.to_owned(),
                };
            }
            // Mirror the HTTP restart endpoint: signal the restart watcher on
            // unix, refuse elsewhere.
            if cfg!(unix) {
                self.restart_signal.notify_one();
                RestartResponse {
                    request_id: request.request_id,
                    accepted: true,
                    error: String::new(),
                }
            } else {
                RestartResponse {
                    request_id: request.request_id,
                    accepted: false,
                    error: "restart not supported on non-unix platforms".to_owned(),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    /// Writes a minimal valid forwarder config plus its token file into a temp
    /// dir, returning the config path and the tempdir guard (kept alive so the
    /// files survive for the duration of the test).
    fn temp_config(extra: &str) -> (PathBuf, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let token_path = dir.path().join("token");
        std::fs::write(&token_path, "test-token\n").expect("write token");
        let config_path = dir.path().join("forwarder.toml");
        let toml = format!(
            "schema_version = 1\n\n[auth]\ntoken_file = '{}'\n\n[[readers]]\ntarget = \"192.168.1.100\"\n\n{extra}",
            token_path.display()
        );
        std::fs::write(&config_path, toml).expect("write config");
        (config_path, dir)
    }

    fn handler(
        allow: bool,
        config_path: PathBuf,
        restart_signal: Arc<Notify>,
    ) -> ForwarderRemoteConfigHandler {
        let (ui_tx, _ui_rx) = broadcast::channel(16);
        ForwarderRemoteConfigHandler::new(
            allow,
            Arc::new(ConfigState::new(config_path)),
            Arc::new(Mutex::new(SubsystemStatus::ready())),
            ui_tx,
            restart_signal,
        )
    }

    #[tokio::test]
    async fn get_returns_config_json_matching_http_shape() {
        let (config_path, _dir) = temp_config("[control]\nallow_remote_config = true");
        let h = handler(true, config_path.clone(), Arc::new(Notify::new()));

        let response = h
            .get_config(ConfigGetRequest {
                request_id: "g1".to_owned(),
            })
            .await;

        assert_eq!(response.request_id, "g1");
        assert!(!response.config_json.is_empty());
        // The body must be the same JSON `GET /api/v1/config` returns: the
        // serialized RawConfig.
        let value: serde_json::Value =
            serde_json::from_str(&response.config_json).expect("config_json is valid JSON");
        assert_eq!(value["schema_version"], serde_json::json!(1));
        assert_eq!(
            value["auth"]["token_file"].as_str().map(str::to_owned),
            Some(
                config_path
                    .parent()
                    .unwrap()
                    .join("token")
                    .display()
                    .to_string()
            )
        );
    }

    #[tokio::test]
    async fn get_returns_empty_when_disabled() {
        let (config_path, _dir) = temp_config("[control]\nallow_remote_config = false");
        let h = handler(false, config_path, Arc::new(Notify::new()));

        let response = h
            .get_config(ConfigGetRequest {
                request_id: "g2".to_owned(),
            })
            .await;

        assert!(
            response.config_json.is_empty(),
            "disabled get must return empty config_json"
        );
    }

    #[tokio::test]
    async fn set_persists_change_and_requires_restart() {
        let (config_path, _dir) = temp_config("[general]\n");
        let h = handler(true, config_path.clone(), Arc::new(Notify::new()));

        // Read current config, edit display_name, write it back.
        let config_json = h
            .get_config(ConfigGetRequest {
                request_id: "g".to_owned(),
            })
            .await
            .config_json;

        let mut value: serde_json::Value = serde_json::from_str(&config_json).unwrap();
        value["display_name"] = serde_json::json!("Edited Forwarder");
        let edited = serde_json::to_string(&value).unwrap();

        let response = h
            .set_config(ConfigSetRequest {
                request_id: "s1".to_owned(),
                config_json: edited,
            })
            .await;

        assert!(response.ok, "set should succeed: {}", response.error);
        assert!(response.restart_needed);
        assert!(response.error.is_empty());

        // The on-disk TOML must reflect the change.
        let cfg = crate::config::load_config_from_path(&config_path).expect("reload config");
        assert_eq!(cfg.display_name.as_deref(), Some("Edited Forwarder"));
    }

    #[tokio::test]
    async fn set_rejects_invalid_config() {
        let (config_path, _dir) = temp_config("");
        let h = handler(true, config_path.clone(), Arc::new(Notify::new()));

        // schema_version 2 is rejected by the canonical loader; keep protected
        // sections unchanged so this remains a validation test.
        let mut value: serde_json::Value = serde_json::from_str(
            &h.get_config(ConfigGetRequest {
                request_id: "g".into(),
            })
            .await
            .config_json,
        )
        .unwrap();
        value["schema_version"] = serde_json::json!(2);
        let response = h
            .set_config(ConfigSetRequest {
                request_id: "s2".to_owned(),
                config_json: serde_json::to_string(&value).unwrap(),
            })
            .await;

        assert!(!response.ok);
        assert!(
            response.error.contains("validation") || response.error.contains("schema_version"),
            "unexpected error: {}",
            response.error
        );
        // The original file must be untouched (still schema_version 1, loads).
        crate::config::load_config_from_path(&config_path).expect("original config intact");
    }

    #[tokio::test]
    async fn set_rejected_when_disabled() {
        let (config_path, _dir) = temp_config("");
        let h = handler(false, config_path, Arc::new(Notify::new()));

        let response = h
            .set_config(ConfigSetRequest {
                request_id: "s3".to_owned(),
                config_json: r#"{"schema_version":1}"#.to_owned(),
            })
            .await;

        assert!(!response.ok);
        assert_eq!(response.error, REMOTE_CONFIG_DISABLED);
    }

    #[tokio::test]
    async fn set_rejects_p2p_section_changes() {
        let (config_path, _dir) = temp_config("[p2p]\nenabled = false\n");
        let h = handler(true, config_path.clone(), Arc::new(Notify::new()));
        let mut value: serde_json::Value = serde_json::from_str(
            &h.get_config(ConfigGetRequest {
                request_id: "g".into(),
            })
            .await
            .config_json,
        )
        .unwrap();

        value["p2p"]["static_allowed_receivers"] = serde_json::json!(["ff".repeat(32)]);
        let response = h
            .set_config(ConfigSetRequest {
                request_id: "s".into(),
                config_json: serde_json::to_string(&value).unwrap(),
            })
            .await;

        assert!(!response.ok);
        assert!(
            response.error.contains("p2p"),
            "error must name the protected section: {}",
            response.error
        );
        let cfg = crate::config::load_config_from_path(&config_path).unwrap();
        assert!(cfg.p2p.static_allowed_receivers.is_empty());
    }

    #[tokio::test]
    async fn set_rejects_auth_section_changes() {
        let (config_path, dir) = temp_config("");
        let alternate_token_path = dir.path().join("alternate-token");
        std::fs::write(&alternate_token_path, "alternate-token\n").expect("write alternate token");
        let h = handler(true, config_path.clone(), Arc::new(Notify::new()));
        let mut value: serde_json::Value = serde_json::from_str(
            &h.get_config(ConfigGetRequest {
                request_id: "g".into(),
            })
            .await
            .config_json,
        )
        .unwrap();

        value["auth"]["token_file"] = serde_json::json!(alternate_token_path.display().to_string());
        let response = h
            .set_config(ConfigSetRequest {
                request_id: "s".into(),
                config_json: serde_json::to_string(&value).unwrap(),
            })
            .await;

        assert!(!response.ok);
        assert!(
            response.error.contains("auth"),
            "error must name the protected section: {}",
            response.error
        );
        let cfg = crate::config::load_config_from_path(&config_path).unwrap();
        assert_eq!(cfg.token, "test-token");
    }

    #[tokio::test]
    async fn set_rejects_control_section_changes() {
        let (config_path, _dir) = temp_config("[control]\nallow_remote_config = true\n");
        let h = handler(true, config_path.clone(), Arc::new(Notify::new()));
        let mut value: serde_json::Value = serde_json::from_str(
            &h.get_config(ConfigGetRequest {
                request_id: "g".into(),
            })
            .await
            .config_json,
        )
        .unwrap();

        value["control"]["allow_remote_config"] = serde_json::json!(false);
        let response = h
            .set_config(ConfigSetRequest {
                request_id: "s".into(),
                config_json: serde_json::to_string(&value).unwrap(),
            })
            .await;

        assert!(!response.ok);
        assert!(
            response.error.contains("control"),
            "error must name the protected section: {}",
            response.error
        );
        let cfg = crate::config::load_config_from_path(&config_path).unwrap();
        assert!(cfg.control.allow_remote_config);
    }

    #[tokio::test]
    async fn set_allows_operational_section_changes() {
        let (config_path, dir) = temp_config("");
        let h = handler(true, config_path.clone(), Arc::new(Notify::new()));
        let mut value: serde_json::Value = serde_json::from_str(
            &h.get_config(ConfigGetRequest {
                request_id: "g".into(),
            })
            .await
            .config_json,
        )
        .unwrap();
        let sqlite_path = dir.path().join("edited.sqlite3");

        value["display_name"] = serde_json::json!("Remote Edited Forwarder");
        value["journal"] = serde_json::json!({
            "sqlite_path": sqlite_path.display().to_string(),
            "prune_watermark_pct": 80,
        });
        value["readers"] = serde_json::json!([
            {
                "target": "192.168.1.101",
                "enabled": false,
                "local_fallback_port": 10101,
            }
        ]);
        let response = h
            .set_config(ConfigSetRequest {
                request_id: "s".into(),
                config_json: serde_json::to_string(&value).unwrap(),
            })
            .await;

        assert!(response.ok, "set should succeed: {}", response.error);
        let cfg = crate::config::load_config_from_path(&config_path).unwrap();
        assert_eq!(cfg.display_name.as_deref(), Some("Remote Edited Forwarder"));
        assert_eq!(cfg.journal.sqlite_path, sqlite_path.display().to_string());
        assert_eq!(cfg.journal.prune_watermark_pct, 80);
        assert_eq!(cfg.readers.len(), 1);
        assert_eq!(cfg.readers[0].target, "192.168.1.101");
        assert!(!cfg.readers[0].enabled);
        assert_eq!(cfg.readers[0].local_fallback_port, Some(10101));
    }

    #[tokio::test]
    async fn set_allows_missing_control_to_all_null_control() {
        let (config_path, _dir) = temp_config("");
        let h = handler(true, config_path, Arc::new(Notify::new()));
        let mut value: serde_json::Value = serde_json::from_str(
            &h.get_config(ConfigGetRequest {
                request_id: "g".into(),
            })
            .await
            .config_json,
        )
        .unwrap();

        value["control"] = serde_json::json!({
            "allow_power_actions": null,
            "allow_remote_config": null,
            "allow_reader_control": null,
        });
        let response = h
            .set_config(ConfigSetRequest {
                request_id: "s".into(),
                config_json: serde_json::to_string(&value).unwrap(),
            })
            .await;

        assert!(response.ok, "set should succeed: {}", response.error);
    }

    #[tokio::test]
    async fn set_rejects_populated_control_to_missing_control() {
        let (config_path, _dir) = temp_config("[control]\nallow_remote_config = true\n");
        let h = handler(true, config_path.clone(), Arc::new(Notify::new()));
        let mut value: serde_json::Value = serde_json::from_str(
            &h.get_config(ConfigGetRequest {
                request_id: "g".into(),
            })
            .await
            .config_json,
        )
        .unwrap();

        value.as_object_mut().unwrap().remove("control");
        let response = h
            .set_config(ConfigSetRequest {
                request_id: "s".into(),
                config_json: serde_json::to_string(&value).unwrap(),
            })
            .await;

        assert!(!response.ok);
        assert!(
            response.error.contains("control"),
            "error must name the protected section: {}",
            response.error
        );
        let cfg = crate::config::load_config_from_path(&config_path).unwrap();
        assert!(cfg.control.allow_remote_config);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn restart_notifies_signal_without_killing_process() {
        let (config_path, _dir) = temp_config("");
        let signal = Arc::new(Notify::new());
        let h = handler(true, config_path, Arc::clone(&signal));

        // A waiter mimics main.rs's restart watcher; the test owns it, so no
        // real process restart happens.
        let waiter = tokio::spawn({
            let signal = Arc::clone(&signal);
            async move { signal.notified().await }
        });

        let response = h
            .restart(RestartRequest {
                request_id: "r1".to_owned(),
            })
            .await;

        assert!(response.accepted);
        assert!(response.error.is_empty());
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("restart signal must fire")
            .expect("waiter task");
    }

    #[tokio::test]
    async fn restart_rejected_when_disabled() {
        let (config_path, _dir) = temp_config("");
        let h = handler(false, config_path, Arc::new(Notify::new()));

        let response = h
            .restart(RestartRequest {
                request_id: "r2".to_owned(),
            })
            .await;

        assert!(!response.accepted);
        assert_eq!(response.error, REMOTE_CONFIG_DISABLED);
    }
}
