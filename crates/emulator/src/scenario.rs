//! Scenario configuration and event generation for the emulator.
//!
//! # YAML Scenario Schema (v1 Frozen)
//!
//! ```yaml
//! mode: reader | forwarder      # required
//! seed: <u64>                   # deterministic RNG seed
//!
//! # Reader mode entries
//! readers:
//!   - ip: "192.168.2.156"
//!     port: 10000
//!     read_type: raw            # raw | fsls
//!     chip_ids: [1000, 1001]
//!     events_per_second: 10
//!     total_events: 500
//!     start_delay_ms: 0
//!     faults:
//!       - type: jitter | disconnect | reconnect_delay
//!         after_events: 100
//!         duration_ms: 2000
//!
//! # Forwarder mode adds:
//! server_url: "wss://timing.example.com/ws/v1/forwarders"
//! token: "<bearer-token>"
//! forwarder_id: "emulated-fwd-1"
//! ```

use ipico_core::read::ReadType;
use serde::{Deserialize, Serialize};

use crate::read_gen::generate_read_for_chip;

/// Base date for deterministic scenario timestamps (IPICO two-digit year, month, day).
const BASE_YEAR: u8 = 26;
const BASE_MONTH: u8 = 1;
const BASE_DAY: u8 = 1;

// ---------------------------------------------------------------------------
// Emulator mode
// ---------------------------------------------------------------------------

/// Operating mode for the emulator scenario.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EmulatorMode {
    /// Bind IP:port and emit IPICO reads (simulates physical reader).
    Reader,
    /// Connect to server WS as a fake forwarder (bypasses real forwarder).
    Forwarder,
}

// ---------------------------------------------------------------------------
// Fault config
// ---------------------------------------------------------------------------

/// A fault injection entry for a reader.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FaultConfig {
    /// Fault type: "jitter", "disconnect", or "reconnect_delay".
    #[serde(rename = "type")]
    pub fault_type: String,
    /// Trigger the fault after this many events have been emitted.
    pub after_events: u64,
    /// Duration of the fault in milliseconds.
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Reader config
// ---------------------------------------------------------------------------

/// Configuration for a single emulated reader in the scenario.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReaderScenarioConfig {
    /// Reader IP address (used as stream_key).
    pub ip: String,
    /// Port to bind (reader mode) or connect to.
    pub port: u16,
    /// "raw" or "fsls".
    pub read_type: String,
    /// Chip IDs to cycle through when generating events.
    pub chip_ids: Vec<u64>,
    /// Target events per second (used for timing in real-time mode).
    pub events_per_second: u32,
    /// Total number of events to generate.
    pub total_events: u64,
    /// Delay before starting event emission (milliseconds).
    pub start_delay_ms: u64,
    /// Fault injection schedule for this reader.
    #[serde(default)]
    pub faults: Vec<FaultConfig>,
}

// ---------------------------------------------------------------------------
// Scenario config
// ---------------------------------------------------------------------------

/// Top-level scenario configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioConfig {
    /// Operating mode.
    pub mode: EmulatorMode,
    /// Deterministic RNG seed for reproducible event generation.
    pub seed: u64,
    /// Readers to emulate.
    #[serde(default)]
    pub readers: Vec<ReaderScenarioConfig>,

    // Forwarder mode only:
    /// Server WebSocket URL (e.g. "wss://...").
    pub server_url: Option<String>,
    /// Bearer token for server authentication.
    pub token: Option<String>,
    /// Forwarder ID to present in hello message.
    pub forwarder_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Parse API
// ---------------------------------------------------------------------------

/// Parse a YAML scenario string into a `ScenarioConfig`.
pub fn load_scenario_from_str(yaml: &str) -> Result<ScenarioConfig, ScenarioError> {
    let scenario: ScenarioConfig =
        serde_yaml::from_str(yaml).map_err(|e| ScenarioError::Parse(e.to_string()))?;
    validate_scenario(&scenario)?;
    Ok(scenario)
}

/// Parse a YAML scenario from a file path.
pub fn load_scenario_from_file(path: &std::path::Path) -> Result<ScenarioConfig, ScenarioError> {
    let content = std::fs::read_to_string(path).map_err(|e| ScenarioError::Io(e.to_string()))?;
    load_scenario_from_str(&content)
}

fn validate_scenario(cfg: &ScenarioConfig) -> Result<(), ScenarioError> {
    for reader in &cfg.readers {
        if reader.chip_ids.is_empty() {
            return Err(ScenarioError::Invalid(format!(
                "reader '{}' must define at least one chip_ids entry",
                reader.ip
            )));
        }

        ReadType::try_from(reader.read_type.as_str()).map_err(|_| {
            ScenarioError::Invalid(format!(
                "reader '{}' has invalid read_type '{}'",
                reader.ip, reader.read_type
            ))
        })?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum ScenarioError {
    Parse(String),
    Io(String),
    Invalid(String),
}

impl std::fmt::Display for ScenarioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScenarioError::Parse(s) => write!(f, "scenario parse error: {}", s),
            ScenarioError::Io(s) => write!(f, "scenario IO error: {}", s),
            ScenarioError::Invalid(s) => write!(f, "invalid scenario: {}", s),
        }
    }
}

impl std::error::Error for ScenarioError {}

// ---------------------------------------------------------------------------
// Generated event type
// ---------------------------------------------------------------------------

/// A single emulated read event (mirrors rt_protocol::ReadEvent fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmulatedEvent {
    /// Reader IP (= stream_key).
    pub reader_ip: String,
    /// Stream epoch (always 1 for fresh scenarios).
    pub stream_epoch: u64,
    /// Monotonically increasing sequence number, starts at 1.
    pub seq: u64,
    /// Simulated reader timestamp (ISO-8601 format).
    pub reader_timestamp: String,
    /// IPICO wire-format raw line: e.g. "aa000000000003e80000260101000000006f"
    pub raw_read_line: String,
    /// Read type: "raw" or "fsls".
    pub read_type: String,
}

// ---------------------------------------------------------------------------
// Deterministic event generation
// ---------------------------------------------------------------------------

/// Generate a deterministic sequence of events for a reader scenario.
///
/// The seed controls chip_id selection order. Events are generated with
/// monotonically increasing seq starting at 1.  Timestamps advance by
/// `1000 / events_per_second` milliseconds per event.
pub fn generate_reader_events(reader: &ReaderScenarioConfig, seed: u64) -> Vec<EmulatedEvent> {
    let mut events = Vec::with_capacity(reader.total_events as usize);

    // Simple seeded LCG for deterministic chip selection
    // LCG constants from Numerical Recipes
    let mut rng_state: u64 = seed.wrapping_add(reader.ip.bytes().map(|b| b as u64).sum::<u64>());

    let ms_per_event = if reader.events_per_second > 0 {
        1000u64 / reader.events_per_second as u64
    } else {
        100
    };

    let mut elapsed_ms: u64 = 0;

    // read_type is validated at scenario parse time.
    let read_type = ReadType::try_from(reader.read_type.as_str())
        .expect("reader.read_type must be validated before event generation");
    let canonical_read_type = read_type.as_str().to_owned();

    for i in 0..reader.total_events {
        let seq = i + 1;

        // Pick chip_id deterministically from the list
        rng_state = lcg_next(rng_state);
        let chip_idx = (rng_state % reader.chip_ids.len() as u64) as usize;
        let chip_id = reader.chip_ids[chip_idx];

        // Generate a timestamp
        let ts = ms_to_timestamp(elapsed_ms);

        // IPICO wire-format raw line via generate_read_for_chip
        let total_secs = elapsed_ms / 1000;
        let centiseconds = ((elapsed_ms % 1000) / 10) as u8;
        let secs = (total_secs % 60) as u8;
        let mins = ((total_secs / 60) % 60) as u8;
        let hours = ((total_secs / 3600) % 24) as u8;
        let raw_read_line = generate_read_for_chip(
            chip_id,
            read_type,
            BASE_YEAR,
            BASE_MONTH,
            BASE_DAY,
            hours,
            mins,
            secs,
            centiseconds,
        );

        events.push(EmulatedEvent {
            reader_ip: reader.ip.clone(),
            stream_epoch: 1,
            seq,
            reader_timestamp: ts,
            raw_read_line,
            read_type: canonical_read_type.clone(),
        });

        elapsed_ms += ms_per_event;
    }

    events
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// LCG: x_{n+1} = (a * x_n + c) mod 2^64
fn lcg_next(state: u64) -> u64 {
    state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
}

/// Convert milliseconds offset to a time string "HH:MM:SS.mmm".
fn ms_to_time_str(ms: u64) -> String {
    let total_secs = ms / 1000;
    let millis = ms % 1000;
    let secs = total_secs % 60;
    let mins = (total_secs / 60) % 60;
    let hours = (total_secs / 3600) % 24;
    format!("{:02}:{:02}:{:02}.{:03}", hours, mins, secs, millis)
}

/// Convert milliseconds offset to an ISO-8601 style timestamp string.
fn ms_to_timestamp(ms: u64) -> String {
    format!(
        "20{BASE_YEAR:02}-{BASE_MONTH:02}-{BASE_DAY:02}T{}Z",
        ms_to_time_str(ms)
    )
}
