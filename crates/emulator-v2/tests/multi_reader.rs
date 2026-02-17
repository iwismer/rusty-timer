//! Integration tests for Task 17: Multi-Reader Deterministic Playback.
//!
//! Tests:
//! 1. YAML scenario parses correctly (reader mode)
//! 2. YAML scenario parses correctly (forwarder mode)
//! 3. Reader mode generates deterministic events with given seed
//! 4. Same seed produces same event sequence (reproducibility)
//! 5. Different seeds produce different event sequences
//! 6. Multi-reader scenario has independent stream identities
//! 7. Events are generated in deterministic order per reader
//! 8. total_events count is respected per reader

use emulator_v2::scenario::{EmulatorMode, load_scenario_from_str};

const SINGLE_READER_YAML: &str = include_str!(
    "../test_assets/scenarios/single_reader.yaml"
);

const MULTI_READER_YAML: &str = include_str!(
    "../test_assets/scenarios/multi_reader.yaml"
);

const FORWARDER_MODE_YAML: &str = include_str!(
    "../test_assets/scenarios/forwarder_mode.yaml"
);

// ---------------------------------------------------------------------------
// Parser tests
// ---------------------------------------------------------------------------

#[test]
fn parse_reader_mode_scenario() {
    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse should succeed");
    assert_eq!(cfg.mode, EmulatorMode::Reader);
    assert_eq!(cfg.seed, 42);
    assert_eq!(cfg.readers.len(), 1);

    let r = &cfg.readers[0];
    assert_eq!(r.ip, "192.168.2.10");
    assert_eq!(r.port, 10010);
    assert_eq!(r.read_type, "raw");
    assert_eq!(r.chip_ids, vec![1000u64, 1001, 1002]);
    assert_eq!(r.events_per_second, 5);
    assert_eq!(r.total_events, 10);
    assert_eq!(r.start_delay_ms, 0);
    assert!(r.faults.is_empty());
}

#[test]
fn parse_multi_reader_scenario() {
    let cfg = load_scenario_from_str(MULTI_READER_YAML).expect("parse should succeed");
    assert_eq!(cfg.mode, EmulatorMode::Reader);
    assert_eq!(cfg.seed, 100);
    assert_eq!(cfg.readers.len(), 2);

    assert_eq!(cfg.readers[0].ip, "192.168.2.10");
    assert_eq!(cfg.readers[0].total_events, 5);
    assert_eq!(cfg.readers[1].ip, "192.168.2.11");
    assert_eq!(cfg.readers[1].total_events, 5);
}

#[test]
fn parse_forwarder_mode_scenario() {
    let cfg = load_scenario_from_str(FORWARDER_MODE_YAML).expect("parse should succeed");
    assert_eq!(cfg.mode, EmulatorMode::Forwarder);
    assert_eq!(cfg.seed, 77);

    // Forwarder-mode fields
    assert_eq!(cfg.server_url.as_deref(), Some("ws://127.0.0.1:9999/ws/v1/forwarders"));
    assert_eq!(cfg.token.as_deref(), Some("test-token-abc"));
    assert_eq!(cfg.forwarder_id.as_deref(), Some("emulated-fwd-1"));
    assert_eq!(cfg.readers.len(), 1);
}

#[test]
fn parse_invalid_mode_returns_error() {
    let bad_yaml = "mode: invalid_mode\nseed: 1\nreaders: []\n";
    let result = load_scenario_from_str(bad_yaml);
    assert!(result.is_err(), "invalid mode must fail to parse");
}

#[test]
fn parse_missing_required_field_returns_error() {
    // Missing 'mode'
    let bad_yaml = "seed: 1\nreaders: []\n";
    let result = load_scenario_from_str(bad_yaml);
    assert!(result.is_err(), "missing mode must fail to parse");
}

// ---------------------------------------------------------------------------
// Deterministic playback tests
// ---------------------------------------------------------------------------

#[test]
fn same_seed_produces_same_events() {
    use emulator_v2::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events1 = generate_reader_events(reader, cfg.seed);
    let events2 = generate_reader_events(reader, cfg.seed);

    assert_eq!(
        events1, events2,
        "same seed must produce identical event sequence"
    );
}

#[test]
fn different_seeds_produce_different_events() {
    use emulator_v2::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events_seed42 = generate_reader_events(reader, 42);
    let events_seed99 = generate_reader_events(reader, 99);

    // With enough events and varied chip_ids, different seeds should differ
    assert_ne!(
        events_seed42, events_seed99,
        "different seeds should produce different event sequences"
    );
}

#[test]
fn event_count_matches_total_events() {
    use emulator_v2::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events = generate_reader_events(reader, cfg.seed);
    assert_eq!(
        events.len(),
        reader.total_events as usize,
        "generated event count must match total_events config"
    );
}

#[test]
fn events_have_reader_ip_as_stream_key() {
    use emulator_v2::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events = generate_reader_events(reader, cfg.seed);
    for event in &events {
        assert_eq!(
            event.reader_ip, reader.ip,
            "event reader_ip must match the reader config IP"
        );
    }
}

#[test]
fn events_seq_is_monotonic() {
    use emulator_v2::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events = generate_reader_events(reader, cfg.seed);
    assert!(!events.is_empty(), "must have events");

    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs[0], 1, "first seq must be 1");
    for i in 1..seqs.len() {
        assert_eq!(seqs[i], seqs[i-1] + 1, "seq must be monotonically increasing by 1");
    }
}

#[test]
fn multi_reader_streams_have_independent_seqs() {
    use emulator_v2::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(MULTI_READER_YAML).expect("parse");
    assert_eq!(cfg.readers.len(), 2);

    let events0 = generate_reader_events(&cfg.readers[0], cfg.seed);
    let events1 = generate_reader_events(&cfg.readers[1], cfg.seed);

    // Each stream must have its own seq starting at 1
    assert_eq!(events0[0].seq, 1, "stream 0 first seq must be 1");
    assert_eq!(events1[0].seq, 1, "stream 1 first seq must be 1");

    // Stream keys must differ
    assert_ne!(
        events0[0].reader_ip, events1[0].reader_ip,
        "multi-reader streams must have distinct reader_ips"
    );
}

#[test]
fn chip_ids_come_from_scenario_config() {
    use emulator_v2::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events = generate_reader_events(reader, cfg.seed);
    let chip_ids_in_config: std::collections::HashSet<String> = reader
        .chip_ids
        .iter()
        .map(|id| id.to_string())
        .collect();

    for event in &events {
        // The raw_read_line should contain a chip_id from the configured list
        let found = chip_ids_in_config.iter().any(|id| event.raw_read_line.contains(id.as_str()));
        assert!(
            found,
            "event raw_read_line '{}' must contain a configured chip_id",
            event.raw_read_line
        );
    }
}
