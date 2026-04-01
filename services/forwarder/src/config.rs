//! Forwarder configuration loading.
//!
//! TOML is the sole config source; no environment variable overrides.
//! Default config path: `/etc/rusty-timer/forwarder.toml`.
//!
//! # Required fields
//! - `schema_version = 1`
//! - `server.base_url`
//! - `auth.token_file`
//! - At least one `[[readers]]` entry
//!
//! # Token file format
//! Raw token string on a single line; trimmed on read.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::Path;

// ---------------------------------------------------------------------------
// Config types (deserialized from TOML)
// ---------------------------------------------------------------------------

/// Top-level forwarder configuration.
#[derive(Debug, Clone)]
pub struct ForwarderConfig {
    pub schema_version: u32,
    /// The bearer token (read from the token file, not the file path).
    pub token: String,
    /// Optional human-readable name for this forwarder (e.g. "Start Line").
    pub display_name: Option<String>,
    pub server: ServerConfig,
    pub journal: JournalConfig,
    pub status_http: StatusHttpConfig,
    pub uplink: UplinkConfig,
    pub control: ControlConfig,
    pub update: UpdateConfig,
    pub ups: UpsConfig,
    pub readers: Vec<ReaderConfig>,
    #[cfg(feature = "eink")]
    pub eink: Option<rt_eink::state::EinkConfig>,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub base_url: String,
    pub forwarders_ws_path: String,
}

#[derive(Debug, Clone)]
pub struct JournalConfig {
    pub sqlite_path: String,
    pub prune_watermark_pct: u8,
}

#[derive(Debug, Clone)]
pub struct StatusHttpConfig {
    pub bind: String,
}

#[derive(Debug, Clone)]
pub struct UplinkConfig {
    pub batch_flush_ms: u64,
    pub batch_max_events: u32,
    /// Seconds to wait for an ack before treating the session as stalled.
    pub ack_timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct ControlConfig {
    pub allow_power_actions: bool,
}

#[derive(Debug, Clone)]
pub struct UpdateConfig {
    pub mode: rt_updater::UpdateMode,
}

#[derive(Debug, Clone)]
pub struct UpsConfig {
    pub enabled: bool,
    pub daemon_addr: String,
    pub poll_interval_secs: u64,
    pub upstream_heartbeat_secs: u64,
}

#[derive(Debug, Clone)]
pub struct ReaderConfig {
    pub target: String,
    pub enabled: bool,
    /// Explicit override; None means use default (10000 + last_octet).
    pub local_fallback_port: Option<u16>,
}

// ---------------------------------------------------------------------------
// Raw TOML deserialization types (with Option for optional fields)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawConfig {
    pub schema_version: Option<u32>,
    pub display_name: Option<String>,
    pub server: Option<RawServerConfig>,
    pub auth: Option<RawAuthConfig>,
    pub journal: Option<RawJournalConfig>,
    pub status_http: Option<RawStatusHttpConfig>,
    pub uplink: Option<RawUplinkConfig>,
    pub control: Option<RawControlConfig>,
    pub update: Option<RawUpdateConfig>,
    pub ups: Option<RawUpsConfig>,
    pub readers: Option<Vec<RawReaderConfig>>,
    #[cfg(feature = "eink")]
    pub eink: Option<rt_eink::state::EinkConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawServerConfig {
    pub base_url: Option<String>,
    pub forwarders_ws_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawAuthConfig {
    pub token_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawJournalConfig {
    pub sqlite_path: Option<String>,
    pub prune_watermark_pct: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawStatusHttpConfig {
    pub bind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawUplinkConfig {
    pub batch_flush_ms: Option<u64>,
    pub batch_max_events: Option<u32>,
    pub ack_timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawControlConfig {
    pub allow_power_actions: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawUpdateConfig {
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawReaderConfig {
    pub target: Option<String>,
    pub enabled: Option<bool>,
    pub local_fallback_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawUpsConfig {
    pub enabled: Option<bool>,
    pub daemon_addr: Option<String>,
    pub poll_interval_secs: Option<u64>,
    pub upstream_heartbeat_secs: Option<u64>,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Load forwarder config from a custom path.
pub fn load_config_from_path(path: &Path) -> Result<ForwarderConfig, ConfigError> {
    let toml_str = std::fs::read_to_string(path)
        .map_err(|e| ConfigError::Io(format!("reading config file '{}': {}", path.display(), e)))?;
    load_config_from_str(&toml_str, path)
}

/// Load forwarder config from the default path `/etc/rusty-timer/forwarder.toml`.
pub fn load_config() -> Result<ForwarderConfig, ConfigError> {
    load_config_from_path(Path::new("/etc/rusty-timer/forwarder.toml"))
}

/// Load forwarder config from a TOML string.
///
/// `config_file_path` is used only to resolve relative paths in the config (not used currently).
/// For tests: pass the path of the temp TOML file (not strictly used for resolution but
/// available for future use). The token_file path from the TOML is used directly.
pub fn load_config_from_str(
    toml_str: &str,
    _config_file_path: &Path,
) -> Result<ForwarderConfig, ConfigError> {
    let raw: RawConfig = toml::from_str(toml_str).map_err(|e| ConfigError::Parse(e.to_string()))?;

    // Validate schema_version
    let schema_version = raw
        .schema_version
        .ok_or_else(|| ConfigError::MissingField("schema_version".to_owned()))?;
    if schema_version != 1 {
        return Err(ConfigError::InvalidValue(format!(
            "schema_version must be 1, got {}",
            schema_version
        )));
    }

    // Validate server
    let raw_server = raw
        .server
        .ok_or_else(|| ConfigError::MissingField("server".to_owned()))?;
    let base_url = raw_server
        .base_url
        .ok_or_else(|| ConfigError::MissingField("server.base_url".to_owned()))?;
    let forwarders_ws_path = raw_server
        .forwarders_ws_path
        .unwrap_or_else(|| "/ws/v1/forwarders".to_owned());

    // Validate auth + read token file
    let raw_auth = raw
        .auth
        .ok_or_else(|| ConfigError::MissingField("auth".to_owned()))?;
    let token_file_path = raw_auth
        .token_file
        .ok_or_else(|| ConfigError::MissingField("auth.token_file".to_owned()))?;
    let token = read_token_file(&token_file_path)?;

    // Journal defaults
    let journal = match raw.journal {
        Some(j) => JournalConfig {
            sqlite_path: j
                .sqlite_path
                .unwrap_or_else(|| "/var/lib/rusty-timer/forwarder.sqlite3".to_owned()),
            prune_watermark_pct: j.prune_watermark_pct.unwrap_or(80),
        },
        None => JournalConfig {
            sqlite_path: "/var/lib/rusty-timer/forwarder.sqlite3".to_owned(),
            prune_watermark_pct: 80,
        },
    };

    // Status HTTP defaults
    let status_http = match raw.status_http {
        Some(s) => StatusHttpConfig {
            bind: s.bind.unwrap_or_else(|| "127.0.0.1:8080".to_owned()),
        },
        None => StatusHttpConfig {
            bind: "127.0.0.1:8080".to_owned(),
        },
    };

    // Uplink defaults
    let uplink = match raw.uplink {
        Some(u) => UplinkConfig {
            batch_flush_ms: u.batch_flush_ms.unwrap_or(100),
            batch_max_events: u.batch_max_events.unwrap_or(50),
            ack_timeout_secs: u.ack_timeout_secs.unwrap_or(30),
        },
        None => UplinkConfig {
            batch_flush_ms: 100,
            batch_max_events: 50,
            ack_timeout_secs: 30,
        },
    };

    // Control defaults
    let control = match raw.control {
        Some(c) => ControlConfig {
            allow_power_actions: c.allow_power_actions.unwrap_or(false),
        },
        None => ControlConfig {
            allow_power_actions: false,
        },
    };

    // Update defaults
    let update = match raw.update {
        Some(u) => {
            let mode = match u.mode {
                Some(m) => serde_json::from_value::<rt_updater::UpdateMode>(
                    serde_json::Value::String(m.clone()),
                )
                .map_err(|_| {
                    ConfigError::InvalidValue(format!(
                        "update.mode must be 'disabled', 'check-only', or 'check-and-download', got '{}'",
                        m
                    ))
                })?,
                None => rt_updater::UpdateMode::default(),
            };
            UpdateConfig { mode }
        }
        None => UpdateConfig {
            mode: rt_updater::UpdateMode::default(),
        },
    };

    // UPS defaults
    let ups = match raw.ups {
        Some(u) => {
            let enabled = u.enabled.unwrap_or(false);
            let daemon_addr = u.daemon_addr.unwrap_or_else(|| "127.0.0.1:8423".to_owned());
            // Validate daemon_addr: must be host:port
            if daemon_addr.parse::<SocketAddr>().is_err() {
                // Try hostname:port via rsplitn
                let parts: Vec<&str> = daemon_addr.rsplitn(2, ':').collect();
                if parts.len() != 2 || parts[0].parse::<u16>().is_err() || parts[1].is_empty() {
                    return Err(ConfigError::InvalidValue(format!(
                        "ups.daemon_addr must be a valid host:port, got '{}'",
                        daemon_addr
                    )));
                }
            }
            let poll_interval_secs = u.poll_interval_secs.unwrap_or(5);
            if !(1..=60).contains(&poll_interval_secs) {
                return Err(ConfigError::InvalidValue(format!(
                    "ups.poll_interval_secs must be 1-60, got {}",
                    poll_interval_secs
                )));
            }
            let upstream_heartbeat_secs = u.upstream_heartbeat_secs.unwrap_or(60);
            if !(10..=300).contains(&upstream_heartbeat_secs) {
                return Err(ConfigError::InvalidValue(format!(
                    "ups.upstream_heartbeat_secs must be 10-300, got {}",
                    upstream_heartbeat_secs
                )));
            }
            UpsConfig {
                enabled,
                daemon_addr,
                poll_interval_secs,
                upstream_heartbeat_secs,
            }
        }
        None => UpsConfig {
            enabled: false,
            daemon_addr: "127.0.0.1:8423".to_owned(),
            poll_interval_secs: 5,
            upstream_heartbeat_secs: 60,
        },
    };

    // Validate readers
    let raw_readers = raw
        .readers
        .ok_or_else(|| ConfigError::MissingField("readers".to_owned()))?;
    if raw_readers.is_empty() {
        return Err(ConfigError::InvalidValue(
            "at least one [[readers]] entry is required".to_owned(),
        ));
    }
    let mut readers = Vec::with_capacity(raw_readers.len());
    for (i, r) in raw_readers.into_iter().enumerate() {
        let target = r
            .target
            .ok_or_else(|| ConfigError::MissingField(format!("readers[{}].target", i)))?;
        readers.push(ReaderConfig {
            target,
            enabled: r.enabled.unwrap_or(true),
            local_fallback_port: r.local_fallback_port,
        });
    }

    Ok(ForwarderConfig {
        schema_version,
        token,
        display_name: raw.display_name,
        server: ServerConfig {
            base_url,
            forwarders_ws_path,
        },
        journal,
        status_http,
        uplink,
        control,
        update,
        ups,
        readers,
        #[cfg(feature = "eink")]
        eink: raw.eink,
    })
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ConfigError {
    Io(String),
    Parse(String),
    MissingField(String),
    InvalidValue(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Io(s) => write!(f, "IO error: {}", s),
            ConfigError::Parse(s) => write!(f, "Parse error: {}", s),
            ConfigError::MissingField(s) => write!(f, "Missing required field: {}", s),
            ConfigError::InvalidValue(s) => write!(f, "Invalid config value: {}", s),
        }
    }
}

impl std::error::Error for ConfigError {}

// ---------------------------------------------------------------------------
// Token file reader
// ---------------------------------------------------------------------------

fn read_token_file(path: &str) -> Result<String, ConfigError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| ConfigError::Io(format!("reading token file '{}': {}", path, e)))?;
    Ok(content.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns (toml_string, _tempdir_guard). The caller must hold `_tempdir_guard`
    /// alive so the token file is not deleted before config parsing reads it.
    fn minimal_toml(extra: &str) -> (String, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let token_path = dir.path().join("token");
        std::fs::write(&token_path, "test-token\n").expect("write token");
        let toml = format!(
            r#"
schema_version = 1

[server]
base_url = "wss://example.com"

[auth]
token_file = '{}'

[[readers]]
target = "192.168.1.100"

{extra}
"#,
            token_path.display()
        );
        (toml, dir)
    }

    #[test]
    fn update_section_defaults_to_check_and_download_when_absent() {
        let (toml, _dir) = minimal_toml("");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert_eq!(cfg.update.mode, rt_updater::UpdateMode::CheckAndDownload);
    }

    #[test]
    fn update_section_parses_disabled() {
        let (toml, _dir) = minimal_toml("[update]\nmode = \"disabled\"");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert_eq!(cfg.update.mode, rt_updater::UpdateMode::Disabled);
    }

    #[test]
    fn update_section_parses_check_only() {
        let (toml, _dir) = minimal_toml("[update]\nmode = \"check-only\"");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert_eq!(cfg.update.mode, rt_updater::UpdateMode::CheckOnly);
    }

    #[test]
    fn update_section_parses_check_and_download() {
        let (toml, _dir) = minimal_toml("[update]\nmode = \"check-and-download\"");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert_eq!(cfg.update.mode, rt_updater::UpdateMode::CheckAndDownload);
    }

    #[test]
    fn update_section_rejects_invalid_mode() {
        let (toml, _dir) = minimal_toml("[update]\nmode = \"yolo\"");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(err.to_string().contains("update.mode"), "error: {err}");
    }

    #[test]
    fn update_section_defaults_mode_when_section_present_but_mode_absent() {
        let (toml, _dir) = minimal_toml("[update]");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert_eq!(cfg.update.mode, rt_updater::UpdateMode::CheckAndDownload);
    }

    #[test]
    fn ups_section_absent_defaults_to_disabled() {
        let (toml, _dir) = minimal_toml("");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert!(!cfg.ups.enabled);
        assert_eq!(cfg.ups.daemon_addr, "127.0.0.1:8423");
        assert_eq!(cfg.ups.poll_interval_secs, 5);
        assert_eq!(cfg.ups.upstream_heartbeat_secs, 60);
    }

    #[test]
    fn ups_section_enabled_with_defaults() {
        let (toml, _dir) = minimal_toml("[ups]\nenabled = true");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert!(cfg.ups.enabled);
        assert_eq!(cfg.ups.daemon_addr, "127.0.0.1:8423");
        assert_eq!(cfg.ups.poll_interval_secs, 5);
        assert_eq!(cfg.ups.upstream_heartbeat_secs, 60);
    }

    #[test]
    fn ups_section_custom_addr() {
        let (toml, _dir) = minimal_toml("[ups]\ndaemon_addr = \"myhost:9999\"");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert_eq!(cfg.ups.daemon_addr, "myhost:9999");
    }

    #[test]
    fn ups_poll_interval_out_of_range_rejected() {
        let (toml, _dir) = minimal_toml("[ups]\npoll_interval_secs = 0");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(
            err.to_string().contains("ups.poll_interval_secs"),
            "error: {err}"
        );

        let (toml, _dir) = minimal_toml("[ups]\npoll_interval_secs = 61");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(
            err.to_string().contains("ups.poll_interval_secs"),
            "error: {err}"
        );
    }

    #[test]
    fn ups_heartbeat_out_of_range_rejected() {
        let (toml, _dir) = minimal_toml("[ups]\nupstream_heartbeat_secs = 9");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(
            err.to_string().contains("ups.upstream_heartbeat_secs"),
            "error: {err}"
        );

        let (toml, _dir) = minimal_toml("[ups]\nupstream_heartbeat_secs = 301");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(
            err.to_string().contains("ups.upstream_heartbeat_secs"),
            "error: {err}"
        );
    }

    #[test]
    fn eink_section_absent_parses_ok() {
        let (toml, _dir) = minimal_toml("");
        let _cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        #[cfg(feature = "eink")]
        assert!(_cfg.eink.is_none());
    }
}
