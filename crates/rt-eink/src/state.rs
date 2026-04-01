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
