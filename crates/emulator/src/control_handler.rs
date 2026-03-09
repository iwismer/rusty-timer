//! Emulated reader control state and response frame building.
//!
//! Provides `EmulatedReaderState` for tracking mutable reader state during
//! control protocol exchanges, and helpers for building valid IPICO control
//! response frames.

use ipico_core::control::ReadMode;

use crate::scenario::ReaderScenarioConfig;

// ---------------------------------------------------------------------------
// Read mode helper
// ---------------------------------------------------------------------------

/// Parse a read mode string into `ReadMode`.
///
/// The `ReadMode` enum lacks a `from_str` method, so we provide a local helper
/// that maps the YAML-friendly strings to enum variants.
fn read_mode_from_str(s: &str) -> Option<ReadMode> {
    match s {
        "raw" => Some(ReadMode::Raw),
        "event" => Some(ReadMode::Event),
        "fsls" => Some(ReadMode::FirstLastSeen),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Emulated reader state
// ---------------------------------------------------------------------------

/// Mutable state for a single emulated reader during a control session.
///
/// Constructed from `ReaderScenarioConfig` with deterministic defaults.
/// Fields mirror the values that a real IPICO reader exposes through its
/// control protocol (CONFIG3, statistics, extended status, etc.).
pub struct EmulatedReaderState {
    pub reader_ip: String,
    pub fw_version: u8,
    pub hw_code: u8,
    pub hw_identifier: u16,
    pub banner: String,
    pub read_mode: ReadMode,
    pub config3_timeout: u8,
    pub tto_enabled: bool,
    pub recording: bool,
    pub clock_offset_ms: i64,
    pub stored_reads: u32,
    pub downloading: bool,
    pub storage_state: u8,
    pub download_progress: u32,
    pub download_chip_ids: Vec<u64>,
    pub download_seed: u64,
}

impl EmulatedReaderState {
    /// Build initial state from a scenario config and a deterministic seed.
    ///
    /// Precedence for read mode: `initial_read_mode` > `read_type` > Raw.
    pub fn from_config(cfg: &ReaderScenarioConfig, seed: u64) -> Self {
        let read_mode = cfg
            .initial_read_mode
            .as_deref()
            .and_then(read_mode_from_str)
            .or_else(|| read_mode_from_str(&cfg.read_type))
            .unwrap_or(ReadMode::Raw);

        let stored_reads = cfg.stored_reads.unwrap_or(0);

        Self {
            reader_ip: cfg.ip.clone(),
            fw_version: 0x42,
            hw_code: 0x05,
            hw_identifier: 0x5905,
            banner: format!("IPICO Emulator v2.0.0\r\nS/N: EMU-{}\r\n", cfg.ip),
            read_mode,
            config3_timeout: 5,
            tto_enabled: cfg.initial_tto_enabled.unwrap_or(false),
            recording: cfg.initial_recording.unwrap_or(false),
            clock_offset_ms: cfg.clock_offset_ms.unwrap_or(0),
            stored_reads,
            downloading: false,
            storage_state: if stored_reads > 0 { 0x0c } else { 0x01 },
            download_progress: 0,
            download_chip_ids: Vec::new(),
            download_seed: seed,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::ReaderScenarioConfig;

    /// Helper to build a minimal `ReaderScenarioConfig` with defaults.
    fn base_config() -> ReaderScenarioConfig {
        ReaderScenarioConfig {
            ip: "192.168.1.100".to_string(),
            port: 10000,
            read_type: "raw".to_string(),
            chip_ids: vec![1000],
            events_per_second: 10,
            total_events: 100,
            start_delay_ms: 0,
            faults: vec![],
            initial_read_mode: None,
            initial_tto_enabled: None,
            initial_recording: None,
            stored_reads: None,
            clock_offset_ms: None,
        }
    }

    #[test]
    fn state_from_config_defaults() {
        let cfg = base_config();
        let state = EmulatedReaderState::from_config(&cfg, 42);

        assert_eq!(state.read_mode, ReadMode::Raw);
        assert!(!state.tto_enabled);
        assert!(!state.recording);
        assert_eq!(state.stored_reads, 0);
        assert_eq!(state.clock_offset_ms, 0);
        assert!(state.banner.contains("EMU"));
        assert_eq!(state.storage_state, 0x01);
        assert!(!state.downloading);
        assert_eq!(state.download_progress, 0);
        assert_eq!(state.fw_version, 0x42);
        assert_eq!(state.hw_code, 0x05);
        assert_eq!(state.hw_identifier, 0x5905);
        assert_eq!(state.config3_timeout, 5);
    }

    #[test]
    fn state_from_config_with_overrides() {
        let mut cfg = base_config();
        cfg.initial_read_mode = Some("fsls".to_string());
        cfg.initial_tto_enabled = Some(true);
        cfg.initial_recording = Some(true);
        cfg.stored_reads = Some(500);
        cfg.clock_offset_ms = Some(-1500);

        let state = EmulatedReaderState::from_config(&cfg, 99);

        assert_eq!(state.read_mode, ReadMode::FirstLastSeen);
        assert!(state.tto_enabled);
        assert!(state.recording);
        assert_eq!(state.stored_reads, 500);
        assert_eq!(state.clock_offset_ms, -1500);
        assert_eq!(state.storage_state, 0x0c);
        assert_eq!(state.download_seed, 99);
    }
}
