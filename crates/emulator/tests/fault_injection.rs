//! Integration tests for fault injection.
//!
//! Covers: YAML parsing of fault configs, FaultSchedule construction,
//! fault outcome behavior (jitter, disconnect, reconnect_delay,
//! malformed_messages, slow_acks), no-fault baseline, and
//! `until_events` window bounds (transient, permanent, multi-fault).

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
    assert_eq!(fault.until_events, None);
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

#[test]
fn fault_until_events_parses_from_yaml() {
    let yaml = r#"
mode: reader
seed: 1
readers:
  - ip: "192.168.2.4"
    port: 10004
    read_type: raw
    chip_ids: [400]
    events_per_second: 10
    total_events: 50
    start_delay_ms: 0
    faults:
      - type: jitter
        after_events: 5
        duration_ms: 100
        until_events: 8
"#;
    let cfg = load_scenario_from_str(yaml).expect("parse");
    let fault = &cfg.readers[0].faults[0];
    assert_eq!(fault.after_events, 5);
    assert_eq!(fault.until_events, Some(8));
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
            until_events: None,
        },
        FaultConfig {
            fault_type: "disconnect".to_owned(),
            after_events: 20,
            duration_ms: 1000,
            until_events: None,
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
        until_events: None,
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
        until_events: None,
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
        until_events: None,
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
        until_events: None,
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
        until_events: None,
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
            until_events: None,
        },
        FaultConfig {
            fault_type: "disconnect".to_owned(),
            after_events: 5,
            duration_ms: 200,
            until_events: None,
        },
    ];
    let schedule = FaultSchedule::from_fault_configs(&faults);
    let outcome = apply_fault_to_event_emission(&schedule, 5);
    // First defined fault (jitter) wins
    assert_eq!(outcome, FaultOutcome::Jitter { delay_ms: 100 });
}

// ---------------------------------------------------------------------------
// until_events tests
// ---------------------------------------------------------------------------

#[test]
fn until_events_bounds_transient_fault_window() {
    // Fault fires on events 5, 6, 7 but NOT at 8, 9, 10.
    let faults = vec![FaultConfig {
        fault_type: "jitter".to_owned(),
        after_events: 5,
        duration_ms: 100,
        until_events: Some(8),
    }];
    let schedule = FaultSchedule::from_fault_configs(&faults);

    // Before window: normal
    for event_num in 0u64..5 {
        let outcome = apply_fault_to_event_emission(&schedule, event_num);
        assert_eq!(
            outcome,
            FaultOutcome::Normal,
            "event_num={} is before the fault window, expected Normal",
            event_num
        );
    }

    // Inside window [5, 8): fault fires
    for event_num in 5u64..8 {
        let outcome = apply_fault_to_event_emission(&schedule, event_num);
        assert_eq!(
            outcome,
            FaultOutcome::Jitter { delay_ms: 100 },
            "event_num={} is inside the fault window [5,8), expected Jitter",
            event_num
        );
    }

    // At and after upper bound: normal again
    for event_num in 8u64..=10 {
        let outcome = apply_fault_to_event_emission(&schedule, event_num);
        assert_eq!(
            outcome,
            FaultOutcome::Normal,
            "event_num={} is at/after until_events=8, expected Normal",
            event_num
        );
    }
}

#[test]
fn until_events_none_fires_permanently() {
    // A fault with until_events: None fires on every event from after_events onward.
    let faults = vec![FaultConfig {
        fault_type: "jitter".to_owned(),
        after_events: 3,
        duration_ms: 50,
        until_events: None,
    }];
    let schedule = FaultSchedule::from_fault_configs(&faults);

    // Before threshold: normal
    for event_num in 0u64..3 {
        let outcome = apply_fault_to_event_emission(&schedule, event_num);
        assert_eq!(outcome, FaultOutcome::Normal, "event_num={}", event_num);
    }

    // From threshold onward: always fault
    for event_num in 3u64..20 {
        let outcome = apply_fault_to_event_emission(&schedule, event_num);
        assert_eq!(
            outcome,
            FaultOutcome::Jitter { delay_ms: 50 },
            "event_num={} should permanently trigger Jitter when until_events is None",
            event_num
        );
    }
}

#[test]
fn two_faults_first_transient_second_permanent() {
    // First fault: jitter active in [5, 10)
    // Second fault: disconnect active from 10 onward
    // At events 0-4: Normal
    // At events 5-9: Jitter (first fault active, second not yet)
    // At events 10+: Disconnect (first fault expired, second now active)
    let faults = vec![
        FaultConfig {
            fault_type: "jitter".to_owned(),
            after_events: 5,
            duration_ms: 100,
            until_events: Some(10),
        },
        FaultConfig {
            fault_type: "disconnect".to_owned(),
            after_events: 10,
            duration_ms: 500,
            until_events: None,
        },
    ];
    let schedule = FaultSchedule::from_fault_configs(&faults);

    // Before first fault window
    for event_num in 0u64..5 {
        assert_eq!(
            apply_fault_to_event_emission(&schedule, event_num),
            FaultOutcome::Normal,
            "event_num={} should be Normal before any fault",
            event_num
        );
    }

    // Inside first fault window [5, 10): jitter wins (first entry)
    for event_num in 5u64..10 {
        assert_eq!(
            apply_fault_to_event_emission(&schedule, event_num),
            FaultOutcome::Jitter { delay_ms: 100 },
            "event_num={} should trigger Jitter (first fault active)",
            event_num
        );
    }

    // From 10 onward: first fault expired, second fault active
    for event_num in 10u64..15 {
        assert_eq!(
            apply_fault_to_event_emission(&schedule, event_num),
            FaultOutcome::Disconnect {
                reconnect_delay_ms: 500
            },
            "event_num={} should trigger Disconnect (second fault active after first expired)",
            event_num
        );
    }
}
