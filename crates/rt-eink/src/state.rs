use serde::Deserialize;

// ---------------------------------------------------------------------------
// Display state — the bridge between forwarder and display
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct DisplayState {
    pub forwarder_name: Option<String>,
    pub local_ip: Option<String>,
    pub server_connected: bool,
    pub readers: Vec<ReaderDisplayState>,
    pub total_reads: u64,
    pub cpu_temp_celsius: Option<f32>,
    pub battery: Option<BatteryState>,
}

impl DisplayState {
    /// Initial state before any subsystem has reported in.
    pub fn initial() -> Self {
        Self {
            forwarder_name: None,
            local_ip: None,
            server_connected: false,
            readers: vec![],
            total_reads: 0,
            cpu_temp_celsius: None,
            battery: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReaderConnectionState {
    /// Sort order: Connected < Connecting < Disconnected (connected first).
    Connected = 0,
    Connecting = 1,
    Disconnected = 2,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReaderDisplayState {
    pub ip: String,
    pub state: ReaderConnectionState,
    pub drift_ms: Option<i64>,
    pub session_reads: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BatteryState {
    pub percent: u8,
    pub charging: bool,
}

// ---------------------------------------------------------------------------
// Configuration — deserialized from TOML [eink] section
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct EinkConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub model: DisplayModel,
    #[serde(default)]
    pub refresh_mode: RefreshMode,
    #[serde(default = "default_full_refresh_interval")]
    pub full_refresh_interval: u32,
    #[serde(default = "default_min_refresh_interval_ms")]
    pub min_refresh_interval_ms: u64,
    #[serde(default = "default_telemetry_interval_secs")]
    pub telemetry_interval_secs: u64,
}

impl Default for EinkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: DisplayModel::default(),
            refresh_mode: RefreshMode::default(),
            full_refresh_interval: default_full_refresh_interval(),
            min_refresh_interval_ms: default_min_refresh_interval_ms(),
            telemetry_interval_secs: default_telemetry_interval_secs(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
pub enum DisplayModel {
    #[default]
    #[serde(rename = "2in13_v2")]
    Epd2in13V2,
    #[serde(rename = "2in13_v3")]
    Epd2in13V3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshMode {
    #[default]
    Hybrid,
    FullOnly,
    PartialOnly,
}

fn default_true() -> bool {
    true
}
fn default_full_refresh_interval() -> u32 {
    10
}
fn default_min_refresh_interval_ms() -> u64 {
    1000
}
fn default_telemetry_interval_secs() -> u64 {
    30
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_state_has_zero_reads_and_no_readers() {
        let state = DisplayState::initial();
        assert_eq!(state.total_reads, 0);
        assert!(state.readers.is_empty());
        assert!(state.local_ip.is_none());
        assert!(!state.server_connected);
        assert!(state.cpu_temp_celsius.is_none());
        assert!(state.battery.is_none());
    }

    #[test]
    fn reader_connection_state_sort_order() {
        assert!(ReaderConnectionState::Connected < ReaderConnectionState::Connecting);
        assert!(ReaderConnectionState::Connecting < ReaderConnectionState::Disconnected);
    }

    #[test]
    fn eink_config_deserializes_defaults() {
        let config: EinkConfig = toml::from_str("").unwrap();
        assert!(config.enabled);
        assert_eq!(config.model, DisplayModel::Epd2in13V2);
        assert_eq!(config.refresh_mode, RefreshMode::Hybrid);
        assert_eq!(config.full_refresh_interval, 10);
        assert_eq!(config.min_refresh_interval_ms, 1000);
        assert_eq!(config.telemetry_interval_secs, 30);
    }

    #[test]
    fn eink_config_deserializes_all_fields() {
        let toml_str = r#"
            enabled = false
            model = "2in13_v3"
            refresh_mode = "full_only"
            full_refresh_interval = 20
            min_refresh_interval_ms = 500
            telemetry_interval_secs = 60
        "#;
        let config: EinkConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.model, DisplayModel::Epd2in13V3);
        assert_eq!(config.refresh_mode, RefreshMode::FullOnly);
        assert_eq!(config.full_refresh_interval, 20);
        assert_eq!(config.min_refresh_interval_ms, 500);
        assert_eq!(config.telemetry_interval_secs, 60);
    }

    #[test]
    fn eink_config_partial_only_mode() {
        let toml_str = r#"refresh_mode = "partial_only""#;
        let config: EinkConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.refresh_mode, RefreshMode::PartialOnly);
    }
}
