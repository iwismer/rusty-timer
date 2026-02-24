//! Integration tests for fault injection.
//!
//! Reader mode faults: jitter, disconnect, reconnect_delay
//! Forwarder mode faults: malformed_messages, slow_acks
//!
//! Tests:
//! 1. FaultSchedule parses jitter fault from YAML
//! 2. FaultSchedule parses disconnect fault from YAML
//! 3. FaultSchedule parses reconnect_delay fault from YAML
//! 4. Jitter fault delays event emission after trigger point
//! 5. Disconnect fault marks stream as disconnected after trigger point
//! 6. Reconnect delay is configurable
//! 7. Malformed message fault generates unparseable payloads
//! 8. Slow ack fault introduces delay before ack
//! 9. No fault = zero delays, all events emitted

use emulator::faults::{FaultOutcome, FaultSchedule, apply_fault_to_event_emission};
use emulator::scenario::{FaultConfig, load_scenario_from_str};

// ---------------------------------------------------------------------------
// YAML parsing tests for fault configs
// ---------------------------------------------------------------------------

#[test]
fn fault_jitter_parses_from_yaml() {
    let yaml = r#"
mode: reader
seed: 1
readers:
  - ip: "192.168.2.1"
    port: 10001
    read_type: raw
    chip_ids: [100]
    events_per_second: 10
    total_events: 200
    start_delay_ms: 0
    faults:
      - type: jitter
        after_events: 50
        duration_ms: 500
"#;
    let cfg = load_scenario_from_str(yaml).expect("parse");
    let reader = &cfg.readers[0];
    assert_eq!(reader.faults.len(), 1);
    let fault = &reader.faults[0];
    assert_eq!(fault.fault_type, "jitter");
    assert_eq!(fault.after_events, 50);
    assert_eq!(fault.duration_ms, 500);
}

#[test]
fn fault_disconnect_parses_from_yaml() {
    let yaml = r#"
mode: reader
seed: 1
readers:
  - ip: "192.168.2.2"
    port: 10002
    read_type: raw
    chip_ids: [200]
    events_per_second: 10
    total_events: 100
    start_delay_ms: 0
    faults:
      - type: disconnect
        after_events: 30
        duration_ms: 1000
"#;
    let cfg = load_scenario_from_str(yaml).expect("parse");
    assert_eq!(cfg.readers[0].faults[0].fault_type, "disconnect");
    assert_eq!(cfg.readers[0].faults[0].after_events, 30);
}

#[test]
fn fault_reconnect_delay_parses_from_yaml() {
    let yaml = r#"
mode: reader
seed: 1
readers:
  - ip: "192.168.2.3"
    port: 10003
    read_type: raw
    chip_ids: [300]
    events_per_second: 5
    total_events: 50
    start_delay_ms: 0
    faults:
      - type: reconnect_delay
        after_events: 20
        duration_ms: 2000
"#;
    let cfg = load_scenario_from_str(yaml).expect("parse");
    assert_eq!(cfg.readers[0].faults[0].fault_type, "reconnect_delay");
    assert_eq!(cfg.readers[0].faults[0].duration_ms, 2000);
}

// ---------------------------------------------------------------------------
// FaultSchedule construction
// ---------------------------------------------------------------------------

#[test]
fn fault_schedule_from_reader_faults() {
    let faults = vec![
        FaultConfig {
            fault_type: "jitter".to_owned(),
            after_events: 10,
            duration_ms: 500,
        },
        FaultConfig {
            fault_type: "disconnect".to_owned(),
            after_events: 20,
            duration_ms: 1000,
        },
    ];
    let schedule = FaultSchedule::from_fault_configs(&faults);
    assert_eq!(schedule.entries().len(), 2);
}

// ---------------------------------------------------------------------------
// Fault outcome tests
// ---------------------------------------------------------------------------

#[test]
fn no_fault_means_normal_outcome() {
    // With no faults configured, every event emission should be Normal.
    let faults: Vec<FaultConfig> = vec![];
    let schedule = FaultSchedule::from_fault_configs(&faults);

    for event_num in 0u64..10 {
        let outcome = apply_fault_to_event_emission(&schedule, event_num);
        assert_eq!(
            outcome,
            FaultOutcome::Normal,
            "no fault should yield Normal outcome at event_num={}",
            event_num
        );
    }
}

#[test]
fn jitter_fault_triggers_after_threshold() {
    let faults = vec![FaultConfig {
        fault_type: "jitter".to_owned(),
        after_events: 5,
        duration_ms: 200,
    }];
    let schedule = FaultSchedule::from_fault_configs(&faults);

    // Before threshold: normal
    let before = apply_fault_to_event_emission(&schedule, 4);
    assert_eq!(
        before,
        FaultOutcome::Normal,
        "before threshold should be Normal"
    );

    // At/after threshold: Jitter with duration
    let at = apply_fault_to_event_emission(&schedule, 5);
    assert_eq!(
        at,
        FaultOutcome::Jitter { delay_ms: 200 },
        "at threshold should return Jitter"
    );

    let after = apply_fault_to_event_emission(&schedule, 6);
    assert_eq!(
        after,
        FaultOutcome::Jitter { delay_ms: 200 },
        "after threshold should still return Jitter"
    );
}

#[test]
fn disconnect_fault_triggers_after_threshold() {
    let faults = vec![FaultConfig {
        fault_type: "disconnect".to_owned(),
        after_events: 10,
        duration_ms: 500,
    }];
    let schedule = FaultSchedule::from_fault_configs(&faults);

    let before = apply_fault_to_event_emission(&schedule, 9);
    assert_eq!(before, FaultOutcome::Normal);

    let at = apply_fault_to_event_emission(&schedule, 10);
    assert_eq!(
        at,
        FaultOutcome::Disconnect {
            reconnect_delay_ms: 500
        },
        "at threshold should return Disconnect"
    );
}

#[test]
fn reconnect_delay_fault_triggers() {
    let faults = vec![FaultConfig {
        fault_type: "reconnect_delay".to_owned(),
        after_events: 3,
        duration_ms: 1500,
    }];
    let schedule = FaultSchedule::from_fault_configs(&faults);

    let at = apply_fault_to_event_emission(&schedule, 3);
    assert_eq!(
        at,
        FaultOutcome::ReconnectDelay { delay_ms: 1500 },
        "reconnect_delay fault should trigger"
    );
}

#[test]
fn malformed_message_fault_type_recognized() {
    // Forwarder mode faults: malformed_messages and slow_acks
    // These are on FaultConfig with fault_type string.
    let faults = vec![FaultConfig {
        fault_type: "malformed_messages".to_owned(),
        after_events: 0,
        duration_ms: 0,
    }];
    let schedule = FaultSchedule::from_fault_configs(&faults);
    let outcome = apply_fault_to_event_emission(&schedule, 0);
    assert_eq!(
        outcome,
        FaultOutcome::MalformedMessage,
        "malformed_messages fault should return MalformedMessage outcome"
    );
}

#[test]
fn slow_ack_fault_type_recognized() {
    let faults = vec![FaultConfig {
        fault_type: "slow_acks".to_owned(),
        after_events: 0,
        duration_ms: 300,
    }];
    let schedule = FaultSchedule::from_fault_configs(&faults);
    let outcome = apply_fault_to_event_emission(&schedule, 0);
    assert_eq!(
        outcome,
        FaultOutcome::SlowAck { delay_ms: 300 },
        "slow_acks fault should return SlowAck outcome"
    );
}

#[test]
fn earliest_fault_wins_when_multiple_trigger() {
    // When multiple faults trigger at the same threshold, the first one wins.
    let faults = vec![
        FaultConfig {
            fault_type: "jitter".to_owned(),
            after_events: 5,
            duration_ms: 100,
        },
        FaultConfig {
            fault_type: "disconnect".to_owned(),
            after_events: 5,
            duration_ms: 200,
        },
    ];
    let schedule = FaultSchedule::from_fault_configs(&faults);
    let outcome = apply_fault_to_event_emission(&schedule, 5);
    // First defined fault (jitter) wins
    assert_eq!(outcome, FaultOutcome::Jitter { delay_ms: 100 });
}
