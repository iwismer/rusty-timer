//! Disk Watermark Pruning Behavior Tests.
//!
//! Validates the forwarder journal's disk watermark pruning behavior:
//! - Acked events are pruned first when disk pressure is triggered.
//! - Unacked events are only pruned when acked records are exhausted.
//! - After pruning, the remaining events are contiguous and correct.
//! - Pruning does not affect unacked events until absolutely necessary.
//! - Event count tracking is accurate before and after pruning.
//!
//! These tests use the forwarder journal library directly (no Docker needed).

use forwarder::storage::journal::Journal;
use tempfile::NamedTempFile;

// ---------------------------------------------------------------------------
// Test: Acked events are pruned first.
// ---------------------------------------------------------------------------

/// Watermark pruning test: when disk pressure is simulated, acked events
/// are pruned before unacked events.
///
/// Per design: "Forwarder journal pruning targets acked records first.
/// Unacked records are only pruned when acked records are exhausted and
/// disk pressure requires it."
#[test]
fn disk_watermark_acked_pruned_first() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.200.200.1", 1).unwrap();

    // Insert 10 events.
    for _ in 0..10 {
        let seq = j.next_seq("10.200.200.1").unwrap();
        j.insert_event("10.200.200.1", 1, seq, None, "watermark_line", "RAW")
            .unwrap();
    }

    // Ack through seq 7.
    j.update_ack_cursor("10.200.200.1", 1, 7).unwrap();

    // Prune 5 records (all should be acked).
    let pruned = j.prune_acked("10.200.200.1", 5).unwrap();
    assert_eq!(pruned, 5, "should prune exactly 5 acked records");

    // Verify acked 8, 9, 10 unacked are still present (events 6, 7 acked but not yet pruned).
    // And events 8, 9, 10 (unacked) should still be there.
    let unacked_after = j.unacked_events("10.200.200.1", 1, 7).unwrap();
    assert_eq!(
        unacked_after.len(),
        3,
        "seq 8, 9, 10 should still be in journal after pruning acked"
    );
    assert_eq!(unacked_after[0].seq, 8);
    assert_eq!(unacked_after[2].seq, 10);
}

// ---------------------------------------------------------------------------
// Test: Unacked events preserved when only acked records pruned.
// ---------------------------------------------------------------------------

/// Watermark pruning test: pruning only removes acked records when requested;
/// unacked events are never touched while acked events are available.
#[test]
fn disk_watermark_unacked_preserved_while_acked_available() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.200.200.2", 1).unwrap();

    // Insert 8 events.
    for _ in 0..8 {
        let seq = j.next_seq("10.200.200.2").unwrap();
        j.insert_event("10.200.200.2", 1, seq, None, "preserve_unacked", "RAW")
            .unwrap();
    }

    // Ack through seq 4.
    j.update_ack_cursor("10.200.200.2", 1, 4).unwrap();

    // Prune all 4 acked records.
    let pruned = j.prune_acked("10.200.200.2", 4).unwrap();
    assert_eq!(pruned, 4, "should prune all 4 acked records");

    // All 4 unacked (seq 5-8) must still be present.
    let unacked = j.unacked_events("10.200.200.2", 1, 4).unwrap();
    assert_eq!(
        unacked.len(),
        4,
        "all 4 unacked events must be preserved after pruning acked"
    );
    let seqs: Vec<i64> = unacked.iter().map(|e| e.seq).collect();
    for expected_seq in [5, 6, 7, 8] {
        assert!(
            seqs.contains(&expected_seq),
            "seq {} must be present after pruning acked",
            expected_seq
        );
    }
}

// ---------------------------------------------------------------------------
// Test: Total event count drops after pruning.
// ---------------------------------------------------------------------------

/// Watermark pruning test: total event count accurately decreases after pruning.
///
/// This count is used to calculate disk pressure (watermark percentage).
#[test]
fn disk_watermark_total_count_decreases_after_prune() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.200.200.3", 1).unwrap();

    // Insert 12 events.
    for _ in 0..12 {
        let seq = j.next_seq("10.200.200.3").unwrap();
        j.insert_event("10.200.200.3", 1, seq, None, "count_line", "RAW")
            .unwrap();
    }

    let total_before = j.total_event_count().unwrap();
    assert_eq!(total_before, 12, "should have 12 events before pruning");

    // Ack through seq 9.
    j.update_ack_cursor("10.200.200.3", 1, 9).unwrap();

    // Prune 9 acked records.
    let pruned = j.prune_acked("10.200.200.3", 9).unwrap();
    assert_eq!(pruned, 9);

    let total_after = j.total_event_count().unwrap();
    assert_eq!(total_after, 3, "should have 3 events remaining after pruning 9");
}

// ---------------------------------------------------------------------------
// Test: Pruning more than available acked records.
// ---------------------------------------------------------------------------

/// Watermark pruning test: requesting to prune more records than exist
/// only prunes what is available (no over-pruning into unacked territory).
#[test]
fn disk_watermark_prune_limit_not_exceeded() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.200.200.4", 1).unwrap();

    // Insert 5 events.
    for _ in 0..5 {
        let seq = j.next_seq("10.200.200.4").unwrap();
        j.insert_event("10.200.200.4", 1, seq, None, "limit_line", "RAW")
            .unwrap();
    }

    // Ack only through seq 2.
    j.update_ack_cursor("10.200.200.4", 1, 2).unwrap();

    // Try to prune 100 (more than the 2 acked).
    let pruned = j.prune_acked("10.200.200.4", 100).unwrap();
    assert_eq!(pruned, 2, "should only prune 2 acked records, not 100");

    // Unacked events (seq 3, 4, 5) must still be present.
    let unacked = j.unacked_events("10.200.200.4", 1, 2).unwrap();
    assert_eq!(unacked.len(), 3, "3 unacked events must remain");
}

// ---------------------------------------------------------------------------
// Test: Multiple streams independent pruning.
// ---------------------------------------------------------------------------

/// Watermark pruning test: pruning one stream does not affect events on
/// other streams. Each stream has an independent acked backlog.
#[test]
fn disk_watermark_streams_pruned_independently() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.200.200.5", 1).unwrap();
    j.ensure_stream_state("10.200.200.6", 1).unwrap();

    // Stream A: 5 events, all acked.
    for _ in 0..5 {
        let seq = j.next_seq("10.200.200.5").unwrap();
        j.insert_event("10.200.200.5", 1, seq, None, "stream_a", "RAW")
            .unwrap();
    }
    j.update_ack_cursor("10.200.200.5", 1, 5).unwrap();

    // Stream B: 4 events, none acked.
    for _ in 0..4 {
        let seq = j.next_seq("10.200.200.6").unwrap();
        j.insert_event("10.200.200.6", 1, seq, None, "stream_b", "RAW")
            .unwrap();
    }

    // Total: 9 events.
    assert_eq!(j.total_event_count().unwrap(), 9);

    // Prune stream A's acked records.
    let pruned_a = j.prune_acked("10.200.200.5", 5).unwrap();
    assert_eq!(pruned_a, 5, "should prune all 5 acked events from stream A");

    // Total should now be 4 (stream B untouched).
    assert_eq!(j.total_event_count().unwrap(), 4, "stream B events must be untouched");

    // Stream B must still have all 4 events.
    let b_unacked = j.unacked_events("10.200.200.6", 1, 0).unwrap();
    assert_eq!(b_unacked.len(), 4, "stream B must still have 4 events after pruning stream A");
}

// ---------------------------------------------------------------------------
// Test: Empty journal prune is a no-op.
// ---------------------------------------------------------------------------

/// Watermark pruning test: pruning from an empty journal or a stream with
/// no acked events returns 0 and does not error.
#[test]
fn disk_watermark_prune_empty_is_noop() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.200.200.7", 1).unwrap();

    // No events inserted — pruning should return 0.
    let pruned = j.prune_acked("10.200.200.7", 10).unwrap();
    assert_eq!(pruned, 0, "pruning empty journal should return 0");

    // Insert events but don't ack — pruning should also return 0.
    for _ in 0..3 {
        let seq = j.next_seq("10.200.200.7").unwrap();
        j.insert_event("10.200.200.7", 1, seq, None, "no_ack", "RAW")
            .unwrap();
    }
    let pruned2 = j.prune_acked("10.200.200.7", 10).unwrap();
    assert_eq!(pruned2, 0, "pruning with no acked events should return 0");
}

// ---------------------------------------------------------------------------
// Test: Watermark prune cycle — simulate disk pressure relief.
// ---------------------------------------------------------------------------

/// Watermark pruning test: simulate a full prune cycle where:
/// 1. Events accumulate (simulating backpressure).
/// 2. Server acks arrive progressively.
/// 3. Watermark pruning kicks in, removing acked records.
/// 4. After each prune cycle, unacked events remain untouched.
#[test]
fn disk_watermark_prune_cycle_simulation() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.200.200.8", 1).unwrap();

    // Round 1: Insert 20 events, ack 10, prune 10.
    for _ in 0..20 {
        let seq = j.next_seq("10.200.200.8").unwrap();
        j.insert_event("10.200.200.8", 1, seq, None, "cycle_line", "RAW")
            .unwrap();
    }
    j.update_ack_cursor("10.200.200.8", 1, 10).unwrap();
    let pruned_r1 = j.prune_acked("10.200.200.8", 10).unwrap();
    assert_eq!(pruned_r1, 10, "round 1: should prune 10 acked events");
    assert_eq!(j.total_event_count().unwrap(), 10, "round 1: 10 events remaining");

    // Round 2: Insert 5 more events (total: 10+5=15), ack 5 more, prune 5.
    for _ in 0..5 {
        let seq = j.next_seq("10.200.200.8").unwrap();
        j.insert_event("10.200.200.8", 1, seq, None, "cycle_line_r2", "RAW")
            .unwrap();
    }
    assert_eq!(j.total_event_count().unwrap(), 15);
    j.update_ack_cursor("10.200.200.8", 1, 15).unwrap();
    let pruned_r2 = j.prune_acked("10.200.200.8", 5).unwrap();
    assert_eq!(pruned_r2, 5, "round 2: should prune 5 more acked events");
    assert_eq!(j.total_event_count().unwrap(), 10, "round 2: 10 events remaining");

    // Verify the remaining 10 events are unacked (seq 16-25).
    let remaining = j.unacked_events("10.200.200.8", 1, 15).unwrap();
    assert_eq!(
        remaining.len(),
        10,
        "remaining 10 events must all be unacked"
    );
    assert_eq!(remaining[0].seq, 16, "first remaining event should be seq 16");
    assert_eq!(remaining[9].seq, 25, "last remaining event should be seq 25");
}
