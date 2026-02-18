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

use serde::Deserialize;
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
    pub readers: Vec<ReaderConfig>,
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
    pub batch_mode: String,
    pub batch_flush_ms: u64,
    pub batch_max_events: u32,
}

#[derive(Debug, Clone)]
pub struct ReaderConfig {
    pub target: String,
    pub read_type: String,
    pub enabled: bool,
    /// Explicit override; None means use default (10000 + last_octet).
    pub local_fallback_port: Option<u16>,
}

// ---------------------------------------------------------------------------
// Raw TOML deserialization types (with Option for optional fields)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawConfig {
    schema_version: Option<u32>,
    display_name: Option<String>,
    server: Option<RawServerConfig>,
    auth: Option<RawAuthConfig>,
    journal: Option<RawJournalConfig>,
    status_http: Option<RawStatusHttpConfig>,
    uplink: Option<RawUplinkConfig>,
    readers: Option<Vec<RawReaderConfig>>,
}

#[derive(Debug, Deserialize)]
struct RawServerConfig {
    base_url: Option<String>,
    forwarders_ws_path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAuthConfig {
    token_file: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawJournalConfig {
    sqlite_path: Option<String>,
    prune_watermark_pct: Option<u8>,
}

#[derive(Debug, Deserialize)]
struct RawStatusHttpConfig {
    bind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawUplinkConfig {
    batch_mode: Option<String>,
    batch_flush_ms: Option<u64>,
    batch_max_events: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct RawReaderConfig {
    target: Option<String>,
    read_type: Option<String>,
    enabled: Option<bool>,
    local_fallback_port: Option<u16>,
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
            bind: s.bind.unwrap_or_else(|| "0.0.0.0:8080".to_owned()),
        },
        None => StatusHttpConfig {
            bind: "0.0.0.0:8080".to_owned(),
        },
    };

    // Uplink defaults
    let uplink = match raw.uplink {
        Some(u) => UplinkConfig {
            batch_mode: u.batch_mode.unwrap_or_else(|| "immediate".to_owned()),
            batch_flush_ms: u.batch_flush_ms.unwrap_or(100),
            batch_max_events: u.batch_max_events.unwrap_or(50),
        },
        None => UplinkConfig {
            batch_mode: "immediate".to_owned(),
            batch_flush_ms: 100,
            batch_max_events: 50,
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
            read_type: r.read_type.unwrap_or_else(|| "raw".to_owned()),
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
        readers,
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
