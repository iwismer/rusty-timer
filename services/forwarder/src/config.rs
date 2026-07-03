//! Forwarder configuration loading.
//!
//! TOML is the sole config source; no environment variable overrides.
//! Default config path: `/etc/rusty-timer/forwarder.toml`.
//!
//! # Required fields
//! - `schema_version = 1`
//! - `auth.token_file`
//! - At least one `[[readers]]` entry
//!
//! # Token file format
//! Raw token string on a single line; trimmed on read.

use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::Path;

const DEFAULT_MIN_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;
const DEFAULT_MAX_RETENTION_SECS: u64 = 30 * 24 * 60 * 60;
const DEFAULT_EMERGENCY_FREE_DISK_BYTES: u64 = 1_000_000_000;
const DEFAULT_EMERGENCY_MAX_ROWS: i64 = 1_000_000;

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
    pub journal: JournalConfig,
    pub status_http: StatusHttpConfig,
    pub control: ControlConfig,
    pub update: UpdateConfig,
    pub ups: UpsConfig,
    pub p2p: P2pConfig,
    pub readers: Vec<ReaderConfig>,
    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub screen: Option<rt_screen::state::ScreenConfig>,
}

#[derive(Debug, Clone)]
pub struct JournalConfig {
    pub sqlite_path: String,
    pub prune_watermark_pct: u8,
    pub min_retention_secs: u64,
    pub max_retention_secs: u64,
    pub emergency_free_disk_bytes: u64,
    pub emergency_max_rows: i64,
}

impl JournalConfig {
    pub fn retention_policy(&self) -> crate::storage::journal::RetentionPolicy {
        crate::storage::journal::RetentionPolicy {
            min_retention_ms: secs_to_ms_i64(self.min_retention_secs),
            max_retention_ms: secs_to_ms_i64(self.max_retention_secs),
            emergency_free_disk_bytes: self.emergency_free_disk_bytes,
            emergency_max_rows: self.emergency_max_rows,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StatusHttpConfig {
    pub bind: String,
}

#[derive(Debug, Clone)]
pub struct ControlConfig {
    pub allow_power_actions: bool,
    /// Gates remote forwarder config get/set/restart over P2P (Phase 4/5).
    /// Defaults to `true` per the product decision to allow remote config by
    /// default.
    pub allow_remote_config: bool,
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
pub struct P2pConfig {
    pub enabled: bool,
    pub secret_key_path: Option<String>,
    pub secret_key_seed_hex: Option<String>,
    pub bind_addr_v4: String,
    pub relay_disabled: bool,
    pub discovery_disabled: bool,
    pub max_concurrent_bidi_streams: Option<u32>,
    pub static_allowed_receivers: Vec<String>,
    pub allowlist_cache_path: Option<String>,
    pub server_url: Option<String>,
    pub server_token_file: Option<String>,
    /// Writable path where the server-minted per-device token is cached. The
    /// `server_token_file` is the (read-only) bootstrap voucher; once a token is
    /// minted it is persisted here and used for all server calls. Defaults to a
    /// sibling of the secret-key path when unset.
    pub device_token_file: Option<String>,
    pub allowlist_poll_interval_secs: u64,
    pub allowlist_request_timeout_secs: u64,
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
    pub auth: Option<RawAuthConfig>,
    pub journal: Option<RawJournalConfig>,
    pub status_http: Option<RawStatusHttpConfig>,
    pub control: Option<RawControlConfig>,
    pub update: Option<RawUpdateConfig>,
    pub ups: Option<RawUpsConfig>,
    pub p2p: Option<RawP2pConfig>,
    pub readers: Option<Vec<RawReaderConfig>>,
    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub eink: Option<rt_screen::state::EinkConfig>,
    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub screen: Option<rt_screen::state::ScreenConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawAuthConfig {
    pub token_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawJournalConfig {
    pub sqlite_path: Option<String>,
    pub prune_watermark_pct: Option<u8>,
    pub min_retention: Option<String>,
    pub max_retention: Option<String>,
    pub emergency_free_disk_bytes: Option<u64>,
    pub emergency_max_rows: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawStatusHttpConfig {
    pub bind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawControlConfig {
    pub allow_power_actions: Option<bool>,
    pub allow_remote_config: Option<bool>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawP2pConfig {
    pub enabled: Option<bool>,
    pub secret_key_path: Option<String>,
    pub secret_key_seed_hex: Option<String>,
    pub bind_addr_v4: Option<String>,
    pub relay_disabled: Option<bool>,
    pub discovery_disabled: Option<bool>,
    pub max_concurrent_bidi_streams: Option<u32>,
    pub static_allowed_receivers: Option<Vec<String>>,
    pub allowlist_cache_path: Option<String>,
    pub server_url: Option<String>,
    pub server_token_file: Option<String>,
    pub device_token_file: Option<String>,
    pub allowlist_poll_interval_secs: Option<u64>,
    pub allowlist_request_timeout_secs: Option<u64>,
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
            min_retention_secs: parse_retention_duration_secs(
                j.min_retention.as_deref(),
                DEFAULT_MIN_RETENTION_SECS,
                "journal.min_retention",
            )?,
            max_retention_secs: parse_retention_duration_secs(
                j.max_retention.as_deref(),
                DEFAULT_MAX_RETENTION_SECS,
                "journal.max_retention",
            )?,
            emergency_free_disk_bytes: j
                .emergency_free_disk_bytes
                .unwrap_or(DEFAULT_EMERGENCY_FREE_DISK_BYTES),
            emergency_max_rows: j.emergency_max_rows.unwrap_or(DEFAULT_EMERGENCY_MAX_ROWS),
        },
        None => JournalConfig {
            sqlite_path: "/var/lib/rusty-timer/forwarder.sqlite3".to_owned(),
            prune_watermark_pct: 80,
            min_retention_secs: DEFAULT_MIN_RETENTION_SECS,
            max_retention_secs: DEFAULT_MAX_RETENTION_SECS,
            emergency_free_disk_bytes: DEFAULT_EMERGENCY_FREE_DISK_BYTES,
            emergency_max_rows: DEFAULT_EMERGENCY_MAX_ROWS,
        },
    };
    if journal.max_retention_secs < journal.min_retention_secs {
        return Err(ConfigError::InvalidValue(
            "journal.max_retention must be greater than or equal to journal.min_retention"
                .to_owned(),
        ));
    }
    if journal.emergency_max_rows < 1 {
        return Err(ConfigError::InvalidValue(
            "journal.emergency_max_rows must be at least 1".to_owned(),
        ));
    }

    // Status HTTP defaults
    let status_http = match raw.status_http {
        Some(s) => StatusHttpConfig {
            bind: s.bind.unwrap_or_else(|| "127.0.0.1:8080".to_owned()),
        },
        None => StatusHttpConfig {
            bind: "127.0.0.1:8080".to_owned(),
        },
    };

    // Control defaults
    let control = match raw.control {
        Some(c) => ControlConfig {
            allow_power_actions: c.allow_power_actions.unwrap_or(false),
            allow_remote_config: c.allow_remote_config.unwrap_or(true),
        },
        None => ControlConfig {
            allow_power_actions: false,
            allow_remote_config: true,
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

    // P2P defaults. Disabled unless explicitly enabled so a forwarder can run as
    // local-only while still using the same journal and reader ingestion path.
    let p2p = match raw.p2p {
        Some(p) => {
            let allowlist_poll_interval_secs = p.allowlist_poll_interval_secs.unwrap_or(60);
            if allowlist_poll_interval_secs == 0 {
                return Err(ConfigError::InvalidValue(
                    "p2p.allowlist_poll_interval_secs must be at least 1".to_owned(),
                ));
            }
            let allowlist_request_timeout_secs = p.allowlist_request_timeout_secs.unwrap_or(10);
            if allowlist_request_timeout_secs == 0 {
                return Err(ConfigError::InvalidValue(
                    "p2p.allowlist_request_timeout_secs must be at least 1".to_owned(),
                ));
            }
            if p.secret_key_path.is_some() && p.secret_key_seed_hex.is_some() {
                return Err(ConfigError::InvalidValue(
                    "p2p.secret_key_path and p2p.secret_key_seed_hex are mutually exclusive"
                        .to_owned(),
                ));
            }
            if let Some(seed) = p.secret_key_seed_hex.as_deref() {
                validate_hex_seed(seed, "p2p.secret_key_seed_hex")?;
            }
            let enabled = p.enabled.unwrap_or(false);
            // The forwarder reserves one bidirectional stream for the long-lived
            // control plane and needs at least one more for data subscriptions,
            // so a P2P-enabled endpoint must permit at least two concurrent
            // bidirectional streams.
            if enabled
                && p.max_concurrent_bidi_streams
                    .is_some_and(|max_streams| max_streams < 2)
            {
                return Err(ConfigError::InvalidValue(
                    "p2p.max_concurrent_bidi_streams must be at least 2 when p2p is enabled \
                     (one stream is reserved for control, one for data)"
                        .to_owned(),
                ));
            }
            P2pConfig {
                enabled,
                secret_key_path: p.secret_key_path,
                secret_key_seed_hex: p.secret_key_seed_hex,
                bind_addr_v4: p.bind_addr_v4.unwrap_or_else(|| "0.0.0.0:0".to_owned()),
                relay_disabled: p.relay_disabled.unwrap_or(false),
                discovery_disabled: p.discovery_disabled.unwrap_or(false),
                max_concurrent_bidi_streams: p.max_concurrent_bidi_streams,
                static_allowed_receivers: p.static_allowed_receivers.unwrap_or_default(),
                allowlist_cache_path: p.allowlist_cache_path,
                server_url: p.server_url,
                server_token_file: p.server_token_file,
                device_token_file: p.device_token_file,
                allowlist_poll_interval_secs,
                allowlist_request_timeout_secs,
            }
        }
        None => P2pConfig {
            enabled: false,
            secret_key_path: None,
            secret_key_seed_hex: None,
            bind_addr_v4: "0.0.0.0:0".to_owned(),
            relay_disabled: false,
            discovery_disabled: false,
            max_concurrent_bidi_streams: None,
            static_allowed_receivers: Vec::new(),
            allowlist_cache_path: None,
            server_url: None,
            server_token_file: None,
            device_token_file: None,
            allowlist_poll_interval_secs: 60,
            allowlist_request_timeout_secs: 10,
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

    // Route both the new `[screen]` block and the legacy `[eink]` block through
    // the effective `screen` config. `[screen]` wins when both are present; a
    // lone `[eink]` migrates to a screen config with the e-ink backend.
    #[cfg(any(feature = "eink", feature = "lcd"))]
    let screen = {
        let screen = match (raw.screen.clone(), raw.eink.clone()) {
            (Some(screen), Some(_legacy)) => {
                tracing::warn!("legacy [eink] config ignored because [screen] is present");
                Some(screen)
            }
            (Some(screen), None) => Some(screen),
            (None, Some(legacy)) => Some(rt_screen::state::ScreenConfig {
                enabled: legacy.enabled,
                backend: rt_screen::state::ScreenBackend::Eink,
                eink: legacy,
                lcd: rt_screen::state::LcdConfig::default(),
            }),
            (None, None) => None,
        };
        if let Some(ref screen) = screen {
            validate_screen_config(screen)?;
        }
        screen
    };

    Ok(ForwarderConfig {
        schema_version,
        token,
        display_name: raw.display_name,
        journal,
        status_http,
        control,
        update,
        ups,
        p2p,
        readers,
        #[cfg(any(feature = "eink", feature = "lcd"))]
        screen,
    })
}

/// Validate an effective `[screen]` config. Shared by the config loader and the
/// `POST /api/v1/config/screen` endpoint so the endpoint cannot persist values
/// that would later make the forwarder fail to boot.
#[cfg(any(feature = "eink", feature = "lcd"))]
pub(crate) fn validate_screen_config(
    screen: &rt_screen::state::ScreenConfig,
) -> Result<(), ConfigError> {
    validate_lcd_config(&screen.lcd)?;
    // The e-ink telemetry check fires for an explicit `backend = "eink"` as well
    // as a migrated legacy `[eink]` block (which always selects the e-ink backend).
    if screen.backend == rt_screen::state::ScreenBackend::Eink
        && screen.eink.telemetry_interval_secs == 0
    {
        return Err(ConfigError::InvalidValue(
            "screen.eink.telemetry_interval_secs must be at least 1".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(any(feature = "eink", feature = "lcd"))]
fn validate_lcd_config(lcd: &rt_screen::state::LcdConfig) -> Result<(), ConfigError> {
    if lcd.telemetry_interval_secs == 0 {
        return Err(ConfigError::InvalidValue(
            "screen.lcd.telemetry_interval_secs must be at least 1".to_owned(),
        ));
    }
    if lcd.spi_clock_hz == 0 {
        return Err(ConfigError::InvalidValue(
            "screen.lcd.spi_clock_hz must be greater than 0".to_owned(),
        ));
    }
    if lcd.dc_pin == lcd.rst_pin
        || lcd.dc_pin == lcd.backlight_pin
        || lcd.rst_pin == lcd.backlight_pin
    {
        return Err(ConfigError::InvalidValue(
            "screen.lcd dc_pin/rst_pin/backlight_pin must be distinct".to_owned(),
        ));
    }
    if lcd.spi_bus != 0 {
        return Err(ConfigError::InvalidValue(
            "screen.lcd only SPI bus 0 is supported".to_owned(),
        ));
    }
    if lcd.spi_chip_select > 1 {
        return Err(ConfigError::InvalidValue(
            "screen.lcd spi_chip_select must be 0 or 1".to_owned(),
        ));
    }
    // The LCD renderer is a fixed 240x320 portrait layout. A landscape rotation
    // makes the effective draw target 320x240, which would clip the layout, so
    // reject it until the renderer is rotation-aware.
    if matches!(
        lcd.rotation,
        rt_screen::state::LcdRotation::Landscape | rt_screen::state::LcdRotation::LandscapeInverted
    ) {
        return Err(ConfigError::InvalidValue(
            "screen.lcd.rotation: landscape is not supported; the layout is portrait \
             (240x320) — use \"portrait\" or \"portrait_inverted\""
                .to_owned(),
        ));
    }
    Ok(())
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

/// Validate journal retention settings as they would be parsed from config.
///
/// Shared by both the TOML load path and the HTTP config-update path so the
/// update endpoint cannot persist values that would later fail to load (invalid
/// duration suffix, min/max inversion, or a zero/negative emergency row cap).
/// `min_retention`/`max_retention` are the raw duration strings (e.g. `"7d"`);
/// `None` means "use the built-in default", matching load behavior. Returns a
/// human-readable error message on the first validation failure.
pub fn validate_retention_settings(
    min_retention: Option<&str>,
    max_retention: Option<&str>,
    emergency_max_rows: Option<i64>,
) -> Result<(), String> {
    let min =
        parse_retention_duration_secs(min_retention, DEFAULT_MIN_RETENTION_SECS, "min_retention")
            .map_err(|e| e.to_string())?;
    let max =
        parse_retention_duration_secs(max_retention, DEFAULT_MAX_RETENTION_SECS, "max_retention")
            .map_err(|e| e.to_string())?;
    if max < min {
        return Err("max_retention must be greater than or equal to min_retention".to_owned());
    }
    if let Some(rows) = emergency_max_rows
        && rows < 1
    {
        return Err("emergency_max_rows must be at least 1".to_owned());
    }
    Ok(())
}

fn validate_hex_seed(value: &str, field: &str) -> Result<(), ConfigError> {
    if value.len() != 64 || !value.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return Err(ConfigError::InvalidValue(format!(
            "{field} must be exactly 64 hex characters"
        )));
    }
    Ok(())
}

fn secs_to_ms_i64(secs: u64) -> i64 {
    secs.checked_mul(1000)
        .and_then(|ms| i64::try_from(ms).ok())
        .unwrap_or(i64::MAX)
}

fn parse_retention_duration_secs(
    value: Option<&str>,
    default_secs: u64,
    field: &str,
) -> Result<u64, ConfigError> {
    let Some(raw) = value else {
        return Ok(default_secs);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ConfigError::InvalidValue(format!(
            "{field} must not be empty"
        )));
    }

    let (number, multiplier) = if let Some(days) = raw.strip_suffix('d') {
        (days, 24 * 60 * 60)
    } else if let Some(hours) = raw.strip_suffix('h') {
        (hours, 60 * 60)
    } else if let Some(minutes) = raw.strip_suffix('m') {
        (minutes, 60)
    } else if let Some(seconds) = raw.strip_suffix('s') {
        (seconds, 1)
    } else {
        return Err(ConfigError::InvalidValue(format!(
            "{field} must use a duration suffix like '7d', '12h', '30m', or '60s'"
        )));
    };

    let amount = number.parse::<u64>().map_err(|_| {
        ConfigError::InvalidValue(format!(
            "{field} must use a positive integer duration, got '{raw}'"
        ))
    })?;
    if amount == 0 {
        return Err(ConfigError::InvalidValue(format!(
            "{field} must be greater than zero"
        )));
    }
    amount.checked_mul(multiplier).ok_or_else(|| {
        ConfigError::InvalidValue(format!("{field} is too large to represent in seconds"))
    })
}

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
    fn p2p_section_absent_defaults_to_disabled() {
        let (toml, _dir) = minimal_toml("");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert!(!cfg.p2p.enabled);
        assert_eq!(cfg.p2p.bind_addr_v4, "0.0.0.0:0");
        assert!(cfg.p2p.static_allowed_receivers.is_empty());
    }

    #[test]
    fn p2p_section_parses_loopback_deterministic_options() {
        let (toml, _dir) = minimal_toml(
            r#"
[p2p]
enabled = true
secret_key_seed_hex = "0101010101010101010101010101010101010101010101010101010101010101"
bind_addr_v4 = "127.0.0.1:0"
relay_disabled = true
discovery_disabled = true
max_concurrent_bidi_streams = 64
static_allowed_receivers = ["receiver-endpoint-id"]
allowlist_cache_path = "/tmp/forwarder-p2p-allowlist.cache"
server_url = "http://127.0.0.1:9999"
server_token_file = "/tmp/thin-token"
allowlist_poll_interval_secs = 5
"#,
        );
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert!(cfg.p2p.enabled);
        assert_eq!(
            cfg.p2p.secret_key_seed_hex.as_deref(),
            Some("0101010101010101010101010101010101010101010101010101010101010101")
        );
        assert_eq!(cfg.p2p.bind_addr_v4, "127.0.0.1:0");
        assert!(cfg.p2p.relay_disabled);
        assert!(cfg.p2p.discovery_disabled);
        assert_eq!(cfg.p2p.max_concurrent_bidi_streams, Some(64));
        assert_eq!(
            cfg.p2p.static_allowed_receivers,
            vec!["receiver-endpoint-id"]
        );
        assert_eq!(
            cfg.p2p.allowlist_cache_path.as_deref(),
            Some("/tmp/forwarder-p2p-allowlist.cache")
        );
        assert_eq!(cfg.p2p.server_url.as_deref(), Some("http://127.0.0.1:9999"));
        assert_eq!(
            cfg.p2p.server_token_file.as_deref(),
            Some("/tmp/thin-token")
        );
        assert_eq!(cfg.p2p.allowlist_poll_interval_secs, 5);
    }

    #[test]
    fn p2p_enabled_rejects_max_concurrent_bidi_streams_below_two() {
        let (toml, _dir) = minimal_toml(
            r#"
[p2p]
enabled = true
static_allowed_receivers = ["receiver-endpoint-id"]
max_concurrent_bidi_streams = 1
"#,
        );
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        match err {
            ConfigError::InvalidValue(msg) => {
                assert!(
                    msg.contains("max_concurrent_bidi_streams"),
                    "unexpected message: {msg}"
                );
            }
            other => panic!("expected InvalidValue, got {other:?}"),
        }
    }

    #[test]
    fn p2p_disabled_ignores_low_max_concurrent_bidi_streams() {
        let (toml, _dir) = minimal_toml(
            r#"
[p2p]
enabled = false
max_concurrent_bidi_streams = 1
"#,
        );
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert_eq!(cfg.p2p.max_concurrent_bidi_streams, Some(1));
    }

    #[test]
    fn p2p_enabled_accepts_max_concurrent_bidi_streams_of_two() {
        let (toml, _dir) = minimal_toml(
            r#"
[p2p]
enabled = true
static_allowed_receivers = ["receiver-endpoint-id"]
max_concurrent_bidi_streams = 2
"#,
        );
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert_eq!(cfg.p2p.max_concurrent_bidi_streams, Some(2));
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
    fn control_section_absent_defaults_allow_remote_config_true() {
        let (toml, _dir) = minimal_toml("");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert!(cfg.control.allow_remote_config);
        // Existing default for power actions is unchanged.
        assert!(!cfg.control.allow_power_actions);
    }

    #[test]
    fn control_section_present_without_allow_remote_config_defaults_true() {
        let (toml, _dir) = minimal_toml("[control]\nallow_power_actions = true");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert!(cfg.control.allow_remote_config);
        assert!(cfg.control.allow_power_actions);
    }

    #[test]
    fn control_section_honors_explicit_allow_remote_config_false() {
        let (toml, _dir) = minimal_toml("[control]\nallow_remote_config = false");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert!(!cfg.control.allow_remote_config);
        // Power actions still default to false when unspecified.
        assert!(!cfg.control.allow_power_actions);
    }

    #[test]
    fn eink_section_absent_parses_ok() {
        let (toml, _dir) = minimal_toml("");
        let _cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        #[cfg(any(feature = "eink", feature = "lcd"))]
        assert!(_cfg.screen.is_none());
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn eink_telemetry_interval_zero_rejected() {
        let (toml, _dir) = minimal_toml("[eink]\ntelemetry_interval_secs = 0");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(
            err.to_string().contains("telemetry_interval_secs"),
            "error: {err}"
        );
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_section_absent_parses_none() {
        let (toml, _dir) = minimal_toml("");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert!(cfg.screen.is_none());
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_section_present_defaults_to_lcd_enabled() {
        let (toml, _dir) = minimal_toml("[screen]");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        let screen = cfg.screen.expect("screen config");
        assert!(screen.enabled);
        assert_eq!(screen.backend, rt_screen::state::ScreenBackend::Lcd);
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_lcd_section_parses_default_pins() {
        let (toml, _dir) = minimal_toml("[screen.lcd]");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        let lcd = cfg.screen.expect("screen config").lcd;
        assert_eq!(lcd.dc_pin, 25);
        assert_eq!(lcd.rst_pin, 27);
        assert_eq!(lcd.backlight_pin, 18);
        assert_eq!(lcd.spi_bus, 0);
        assert_eq!(lcd.spi_chip_select, 0);
        assert_eq!(lcd.spi_clock_hz, 32_000_000);
        assert_eq!(lcd.min_refresh_interval_ms, 250);
        assert_eq!(lcd.telemetry_interval_secs, 10);
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn legacy_eink_only_migrates_to_screen_eink_backend() {
        let (toml, _dir) = minimal_toml("[eink]");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert_eq!(
            cfg.screen.expect("screen config").backend,
            rt_screen::state::ScreenBackend::Eink
        );
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn legacy_eink_disabled_preserves_screen_enabled_false() {
        let (toml, _dir) = minimal_toml("[eink]\nenabled = false");
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        assert!(!cfg.screen.expect("screen config").enabled);
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_section_wins_over_legacy_eink() {
        let (toml, _dir) = minimal_toml(
            r#"
[screen]
backend = "lcd"
[eink]
enabled = false
"#,
        );
        let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
        let screen = cfg.screen.expect("screen config");
        assert!(screen.enabled);
        assert_eq!(screen.backend, rt_screen::state::ScreenBackend::Lcd);
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_lcd_telemetry_interval_zero_rejected() {
        let (toml, _dir) = minimal_toml("[screen.lcd]\ntelemetry_interval_secs = 0");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(
            err.to_string()
                .contains("screen.lcd.telemetry_interval_secs"),
            "error: {err}"
        );
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_lcd_spi_clock_zero_rejected() {
        let (toml, _dir) = minimal_toml("[screen.lcd]\nspi_clock_hz = 0");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(
            err.to_string().contains("screen.lcd.spi_clock_hz"),
            "error: {err}"
        );
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_lcd_duplicate_pins_rejected() {
        let (toml, _dir) = minimal_toml("[screen.lcd]\ndc_pin = 25\nrst_pin = 25");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(
            err.to_string().contains("dc_pin/rst_pin/backlight_pin"),
            "error: {err}"
        );
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_invalid_backend_string_rejected() {
        let (toml, _dir) = minimal_toml("[screen]\nbackend = \"oled\"");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(err.to_string().contains("backend"), "error: {err}");
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_invalid_rotation_string_rejected() {
        let (toml, _dir) = minimal_toml("[screen.lcd]\nrotation = \"sideways\"");
        let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
        assert!(err.to_string().contains("rotation"), "error: {err}");
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_lcd_landscape_rotation_rejected() {
        // Landscape is a valid enum value but unsupported by the portrait renderer.
        for rot in ["landscape", "landscape_inverted"] {
            let (toml, _dir) = minimal_toml(&format!("[screen.lcd]\nrotation = \"{rot}\""));
            let err = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap_err();
            assert!(
                err.to_string().contains("landscape"),
                "expected landscape rejection for {rot}, got: {err}"
            );
        }
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[test]
    fn screen_lcd_portrait_rotations_accepted() {
        for rot in ["portrait", "portrait_inverted"] {
            let (toml, _dir) = minimal_toml(&format!("[screen.lcd]\nrotation = \"{rot}\""));
            assert!(
                load_config_from_str(&toml, Path::new("/tmp/test.toml")).is_ok(),
                "expected {rot} to be accepted"
            );
        }
    }
}
