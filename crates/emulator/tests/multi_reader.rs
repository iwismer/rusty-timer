//! Integration tests for multi-reader deterministic playback.
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
//! 9. Raw read lines are valid IPICO wire-format
//! 10. Chip IDs map to hex tag IDs in parsed reads

use emulator::scenario::{load_scenario_from_str, EmulatorMode, ReaderScenarioConfig};
use ipico_core::read::ChipRead;
use std::convert::TryFrom;

const SINGLE_READER_YAML: &str = include_str!("../test_assets/scenarios/single_reader.yaml");

const MULTI_READER_YAML: &str = include_str!("../test_assets/scenarios/multi_reader.yaml");

const FORWARDER_MODE_YAML: &str = include_str!("../test_assets/scenarios/forwarder_mode.yaml");

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
    assert_eq!(
        cfg.server_url.as_deref(),
        Some("ws://127.0.0.1:9999/ws/v1/forwarders")
    );
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

#[test]
fn parse_empty_chip_ids_returns_error() {
    let bad_yaml = r#"
mode: reader
seed: 1
readers:
  - ip: "192.168.2.10"
    port: 10010
    read_type: raw
    chip_ids: []
    events_per_second: 1
    total_events: 1
    start_delay_ms: 0
"#;
    let result = load_scenario_from_str(bad_yaml);
    assert!(result.is_err(), "empty chip_ids must fail validation");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("chip_ids"),
        "error should mention chip_ids: {err}"
    );
}

#[test]
fn parse_invalid_read_type_returns_error() {
    let bad_yaml = r#"
mode: reader
seed: 1
readers:
  - ip: "192.168.2.10"
    port: 10010
    read_type: invalid
    chip_ids: [1000]
    events_per_second: 1
    total_events: 1
    start_delay_ms: 0
"#;
    let result = load_scenario_from_str(bad_yaml);
    assert!(result.is_err(), "invalid read_type must fail validation");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("read_type"),
        "error should mention read_type: {err}"
    );
}

#[test]
fn parse_chip_id_larger_than_48_bits_returns_error() {
    let bad_yaml = r#"
mode: reader
seed: 1
readers:
  - ip: "192.168.2.10"
    port: 10010
    read_type: raw
    chip_ids: [281474976710656]
    events_per_second: 1
    total_events: 1
    start_delay_ms: 0
"#;
    let result = load_scenario_from_str(bad_yaml);
    assert!(
        result.is_err(),
        "chip_id above 48 bits must fail validation"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("chip_ids"),
        "error should mention chip_ids: {err}"
    );
}

// ---------------------------------------------------------------------------
// Deterministic playback tests
// ---------------------------------------------------------------------------

#[test]
fn same_seed_produces_same_events() {
    use emulator::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events1 =
        generate_reader_events(reader, cfg.seed).expect("event generation should succeed");
    let events2 =
        generate_reader_events(reader, cfg.seed).expect("event generation should succeed");

    assert_eq!(
        events1, events2,
        "same seed must produce identical event sequence"
    );
}

#[test]
fn different_seeds_produce_different_events() {
    use emulator::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events_seed42 =
        generate_reader_events(reader, 42).expect("event generation should succeed");
    let events_seed99 =
        generate_reader_events(reader, 99).expect("event generation should succeed");

    // With enough events and varied chip_ids, different seeds should differ
    assert_ne!(
        events_seed42, events_seed99,
        "different seeds should produce different event sequences"
    );
}

#[test]
fn event_count_matches_total_events() {
    use emulator::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events = generate_reader_events(reader, cfg.seed).expect("event generation should succeed");
    assert_eq!(
        events.len(),
        reader.total_events as usize,
        "generated event count must match total_events config"
    );
}

#[test]
fn events_have_reader_ip_as_stream_key() {
    use emulator::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events = generate_reader_events(reader, cfg.seed).expect("event generation should succeed");
    for event in &events {
        assert_eq!(
            event.reader_ip, reader.ip,
            "event reader_ip must match the reader config IP"
        );
    }
}

#[test]
fn events_seq_is_monotonic() {
    use emulator::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events = generate_reader_events(reader, cfg.seed).expect("event generation should succeed");
    assert!(!events.is_empty(), "must have events");

    let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs[0], 1, "first seq must be 1");
    for i in 1..seqs.len() {
        assert_eq!(
            seqs[i],
            seqs[i - 1] + 1,
            "seq must be monotonically increasing by 1"
        );
    }
}

#[test]
fn multi_reader_streams_have_independent_seqs() {
    use emulator::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(MULTI_READER_YAML).expect("parse");
    assert_eq!(cfg.readers.len(), 2);

    let events0 =
        generate_reader_events(&cfg.readers[0], cfg.seed).expect("event generation should succeed");
    let events1 =
        generate_reader_events(&cfg.readers[1], cfg.seed).expect("event generation should succeed");

    // Each stream must have its own seq starting at 1
    assert_eq!(events0[0].seq, 1, "stream 0 first seq must be 1");
    assert_eq!(events1[0].seq, 1, "stream 1 first seq must be 1");

    // Stream keys must differ
    assert_ne!(
        events0[0].reader_ip, events1[0].reader_ip,
        "multi-reader streams must have distinct reader_ips"
    );
}

// ---------------------------------------------------------------------------
// IPICO wire-format validation tests
// ---------------------------------------------------------------------------

#[test]
fn raw_read_lines_are_valid_ipico() {
    use emulator::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    let events = generate_reader_events(reader, cfg.seed).expect("event generation should succeed");
    for event in &events {
        let parsed = ChipRead::try_from(event.raw_read_line.as_str());
        assert!(
            parsed.is_ok(),
            "raw_read_line '{}' must be a valid IPICO read (seq={})",
            event.raw_read_line,
            event.seq
        );
    }
}

#[test]
fn chip_ids_map_to_hex_tag_ids() {
    use emulator::scenario::generate_reader_events;

    let cfg = load_scenario_from_str(SINGLE_READER_YAML).expect("parse");
    let reader = &cfg.readers[0];

    // Build the set of expected hex tag IDs from the configured chip_ids
    let expected_tag_ids: std::collections::HashSet<String> = reader
        .chip_ids
        .iter()
        .map(|id| format!("{:012x}", id))
        .collect();

    let events = generate_reader_events(reader, cfg.seed).expect("event generation should succeed");
    for event in &events {
        let parsed =
            ChipRead::try_from(event.raw_read_line.as_str()).expect("raw_read_line must parse");
        assert!(
            expected_tag_ids.contains(&parsed.tag_id),
            "tag_id '{}' must be a hex-formatted chip_id from config (expected one of {:?})",
            parsed.tag_id,
            expected_tag_ids
        );
    }
}

#[test]
fn events_canonicalize_read_type_to_lowercase() {
    use emulator::scenario::generate_reader_events;

    let yaml = r#"
mode: reader
seed: 1
readers:
  - ip: "192.168.2.10"
    port: 10010
    read_type: FSLS
    chip_ids: [1000]
    events_per_second: 1
    total_events: 1
    start_delay_ms: 0
"#;
    let cfg = load_scenario_from_str(yaml).expect("parse");
    let reader = &cfg.readers[0];
    let events = generate_reader_events(reader, cfg.seed).expect("event generation should succeed");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].read_type, "fsls");

    let parsed = ChipRead::try_from(events[0].raw_read_line.as_str()).expect("raw read must parse");
    assert_eq!(parsed.read_type.as_str(), "fsls");
}

#[test]
fn generate_reader_events_returns_error_for_invalid_read_type() {
    use emulator::scenario::generate_reader_events;

    let reader = ReaderScenarioConfig {
        ip: "192.168.2.10".to_owned(),
        port: 10010,
        read_type: "invalid".to_owned(),
        chip_ids: vec![1000],
        events_per_second: 1,
        total_events: 1,
        start_delay_ms: 0,
        faults: Vec::new(),
    };

    let result = generate_reader_events(&reader, 1);
    assert!(result.is_err(), "invalid read_type should return an error");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("read_type"),
        "error should mention read_type: {err}"
    );
}
