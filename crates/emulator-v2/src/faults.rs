//! Fault injection framework for emulator-v2.
//!
//! # Fault types
//!
//! ## Reader mode faults
//! - `jitter`: Add a timing delay after a given event count.
//! - `disconnect`: Simulate a TCP disconnect after a given event count.
//! - `reconnect_delay`: Introduce a configurable pause before reconnecting.
//!
//! ## Forwarder mode faults
//! - `malformed_messages`: Replace event payload with unparseable content.
//! - `slow_acks`: Delay ack responses by `duration_ms`.
//!
//! # Usage
//! ```rust,ignore
//! use emulator_v2::faults::{FaultSchedule, apply_fault_to_event_emission, FaultOutcome};
//! use emulator_v2::scenario::FaultConfig;
//!
//! let faults = vec![FaultConfig { fault_type: "jitter".into(), after_events: 10, duration_ms: 200 }];
//! let schedule = FaultSchedule::from_fault_configs(&faults);
//! let outcome = apply_fault_to_event_emission(&schedule, 10);
//! assert_eq!(outcome, FaultOutcome::Jitter { delay_ms: 200 });
//! ```

use crate::scenario::FaultConfig;

// ---------------------------------------------------------------------------
// Public fault outcome type
// ---------------------------------------------------------------------------

/// The outcome to apply at a given event emission point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultOutcome {
    /// No fault — emit normally.
    Normal,
    /// Add a jitter delay of `delay_ms` milliseconds before/after emitting.
    Jitter { delay_ms: u64 },
    /// Simulate a TCP disconnect; optionally wait `reconnect_delay_ms` ms before reconnect.
    Disconnect { reconnect_delay_ms: u64 },
    /// Pause `delay_ms` ms before reconnecting after disconnect.
    ReconnectDelay { delay_ms: u64 },
    /// Replace the event payload with a malformed/unparseable string.
    MalformedMessage,
    /// Delay the ack response by `delay_ms` ms (forwarder mode).
    SlowAck { delay_ms: u64 },
}

// ---------------------------------------------------------------------------
// FaultSchedule
// ---------------------------------------------------------------------------

/// Parsed and sorted fault trigger schedule for a single reader stream.
pub struct FaultSchedule {
    entries: Vec<FaultEntry>,
}

#[derive(Debug, Clone)]
pub struct FaultEntry {
    pub fault_type: FaultType,
    pub after_events: u64,
    pub duration_ms: u64,
}

/// Internal fault type enum (parsed from string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FaultType {
    Jitter,
    Disconnect,
    ReconnectDelay,
    MalformedMessages,
    SlowAcks,
    /// Unknown fault type — logged and ignored at runtime.
    Unknown(String),
}

impl FaultSchedule {
    /// Build a `FaultSchedule` from a list of `FaultConfig` entries.
    ///
    /// Entries are stored in definition order; the first matching entry wins.
    pub fn from_fault_configs(faults: &[FaultConfig]) -> Self {
        let entries = faults
            .iter()
            .map(|f| FaultEntry {
                fault_type: parse_fault_type(&f.fault_type),
                after_events: f.after_events,
                duration_ms: f.duration_ms,
            })
            .collect();
        FaultSchedule { entries }
    }

    /// Return the parsed fault entries (for inspection in tests).
    pub fn entries(&self) -> &[FaultEntry] {
        &self.entries
    }
}

// ---------------------------------------------------------------------------
// Core fault application function
// ---------------------------------------------------------------------------

/// Determine the fault outcome for a given event emission at `event_num`.
///
/// `event_num` is 0-based (the first event is 0, matching `after_events = 0`).
/// Returns `FaultOutcome::Normal` when no fault is active at this position.
pub fn apply_fault_to_event_emission(schedule: &FaultSchedule, event_num: u64) -> FaultOutcome {
    // Walk entries in definition order; return first match.
    for entry in &schedule.entries {
        if event_num >= entry.after_events {
            return match &entry.fault_type {
                FaultType::Jitter => FaultOutcome::Jitter {
                    delay_ms: entry.duration_ms,
                },
                FaultType::Disconnect => FaultOutcome::Disconnect {
                    reconnect_delay_ms: entry.duration_ms,
                },
                FaultType::ReconnectDelay => FaultOutcome::ReconnectDelay {
                    delay_ms: entry.duration_ms,
                },
                FaultType::MalformedMessages => FaultOutcome::MalformedMessage,
                FaultType::SlowAcks => FaultOutcome::SlowAck {
                    delay_ms: entry.duration_ms,
                },
                FaultType::Unknown(_) => FaultOutcome::Normal,
            };
        }
    }
    FaultOutcome::Normal
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn parse_fault_type(s: &str) -> FaultType {
    match s {
        "jitter" => FaultType::Jitter,
        "disconnect" => FaultType::Disconnect,
        "reconnect_delay" => FaultType::ReconnectDelay,
        "malformed_messages" => FaultType::MalformedMessages,
        "slow_acks" => FaultType::SlowAcks,
        other => FaultType::Unknown(other.to_owned()),
    }
}
