use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Display state — the bridge between forwarder and display
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayState {
    pub forwarder_name: Option<String>,
    pub local_ip: Option<String>,
    pub p2p_connected: bool,
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
            p2p_connected: false,
            readers: vec![],
            total_reads: 0,
            cpu_temp_celsius: None,
            battery: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderConnectionState {
    /// Sort order: Connected < Connecting < Disconnected (connected first).
    Connected = 0,
    Connecting = 1,
    Disconnected = 2,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReaderDisplayState {
    pub ip: String,
    pub state: ReaderConnectionState,
    pub drift_ms: Option<i64>,
    pub session_reads: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BatteryState {
    pub percent: u8,
    pub charging: bool,
}

// ---------------------------------------------------------------------------
// Configuration — deserialized from TOML [eink] section
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EinkConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
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
            refresh_mode: RefreshMode::default(),
            full_refresh_interval: default_full_refresh_interval(),
            min_refresh_interval_ms: default_min_refresh_interval_ms(),
            telemetry_interval_secs: default_telemetry_interval_secs(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub backend: ScreenBackend,
    #[serde(default)]
    pub lcd: LcdConfig,
    #[serde(default)]
    pub eink: EinkConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenBackend {
    #[default]
    Lcd,
    Eink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LcdRotation {
    #[default]
    Portrait,
    Landscape,
    PortraitInverted,
    LandscapeInverted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LcdConfig {
    #[serde(default)]
    pub rotation: LcdRotation,
    #[serde(default = "default_lcd_min_refresh_interval_ms")]
    pub min_refresh_interval_ms: u64,
    #[serde(default = "default_lcd_telemetry_interval_secs")]
    pub telemetry_interval_secs: u64,
    #[serde(default = "default_spi_bus")]
    pub spi_bus: u8,
    #[serde(default = "default_spi_chip_select")]
    pub spi_chip_select: u8,
    #[serde(default = "default_dc_pin")]
    pub dc_pin: u8,
    #[serde(default = "default_rst_pin")]
    pub rst_pin: u8,
    #[serde(default = "default_backlight_pin")]
    pub backlight_pin: u8,
    #[serde(default = "default_spi_clock_hz")]
    pub spi_clock_hz: u32,
}

impl Default for LcdConfig {
    fn default() -> Self {
        Self {
            rotation: LcdRotation::default(),
            min_refresh_interval_ms: default_lcd_min_refresh_interval_ms(),
            telemetry_interval_secs: default_lcd_telemetry_interval_secs(),
            spi_bus: default_spi_bus(),
            spi_chip_select: default_spi_chip_select(),
            dc_pin: default_dc_pin(),
            rst_pin: default_rst_pin(),
            backlight_pin: default_backlight_pin(),
            spi_clock_hz: default_spi_clock_hz(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
    // Force a full refresh after every 10 partial refreshes to clear ghosting; Waveshare
    // warns that refreshing only partially, indefinitely, can permanently damage the panel.
    10
}
fn default_min_refresh_interval_ms() -> u64 {
    // Waveshare recommends a refresh interval of at least 180s. This debounce floor coalesces
    // bursts of state changes so the panel is never refreshed more often than every 3 minutes.
    180_000
}
fn default_telemetry_interval_secs() -> u64 {
    // Periodic redraw to refresh telemetry (CPU temp / battery) and satisfy Waveshare's
    // "refresh at least once every 24 hours" guidance, kept well above the 180s minimum.
    900
}
fn default_lcd_min_refresh_interval_ms() -> u64 {
    250
}
fn default_lcd_telemetry_interval_secs() -> u64 {
    10
}
fn default_spi_bus() -> u8 {
    0
}
fn default_spi_chip_select() -> u8 {
    0
}
fn default_dc_pin() -> u8 {
    25
}
fn default_rst_pin() -> u8 {
    27
}
fn default_backlight_pin() -> u8 {
    18
}
fn default_spi_clock_hz() -> u32 {
    32_000_000
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
        assert!(!state.p2p_connected);
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
        assert_eq!(config.refresh_mode, RefreshMode::Hybrid);
        assert_eq!(config.full_refresh_interval, 10);
        assert_eq!(config.min_refresh_interval_ms, 180_000);
        assert_eq!(config.telemetry_interval_secs, 900);
    }

    #[test]
    fn eink_config_deserializes_all_fields() {
        let toml_str = r#"
            enabled = false
            refresh_mode = "full_only"
            full_refresh_interval = 20
            min_refresh_interval_ms = 500
            telemetry_interval_secs = 60
        "#;
        let config: EinkConfig = toml::from_str(toml_str).unwrap();
        assert!(!config.enabled);
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

    #[test]
    fn eink_config_rejects_model_field() {
        let toml_str = r#"model = "2in13_v4""#;
        let err = toml::from_str::<EinkConfig>(toml_str).unwrap_err();
        assert!(err.to_string().contains("unknown field"), "error: {err}");
        assert!(err.to_string().contains("model"), "error: {err}");
    }

    #[test]
    fn screen_config_deserializes_defaults() {
        let config: ScreenConfig = toml::from_str("").unwrap();
        assert!(config.enabled);
        assert_eq!(config.backend, ScreenBackend::Lcd);
    }

    #[test]
    fn screen_lcd_config_deserializes_default_hardware_values() {
        let config: ScreenConfig = toml::from_str("[lcd]").unwrap();
        assert_eq!(config.lcd.dc_pin, 25);
        assert_eq!(config.lcd.rst_pin, 27);
        assert_eq!(config.lcd.backlight_pin, 18);
        assert_eq!(config.lcd.spi_bus, 0);
        assert_eq!(config.lcd.spi_chip_select, 0);
        assert_eq!(config.lcd.spi_clock_hz, 32_000_000);
        assert_eq!(config.lcd.min_refresh_interval_ms, 250);
        assert_eq!(config.lcd.telemetry_interval_secs, 10);
    }

    #[test]
    fn screen_lcd_rotation_defaults_to_portrait() {
        let config: ScreenConfig = toml::from_str("[lcd]").unwrap();
        assert_eq!(config.lcd.rotation, LcdRotation::Portrait);
    }

    #[test]
    fn screen_config_rejects_invalid_backend() {
        let err = toml::from_str::<ScreenConfig>(r#"backend = "oled""#).unwrap_err();
        assert!(err.to_string().contains("backend"), "error: {err}");
    }

    #[test]
    fn screen_lcd_config_rejects_invalid_rotation() {
        let err = toml::from_str::<ScreenConfig>(
            r#"[lcd]
rotation = "sideways""#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("rotation"), "error: {err}");
    }
}
