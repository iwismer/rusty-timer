/// Tests for replay/resume behavior after disconnect/reconnect.
///
/// Validates:
/// - After disconnect, unsent/unacked events are replayed from the correct cursor
/// - Replay starts from acked_cursor+1 (not from seq 1)
/// - Events across epoch boundaries are replayed correctly
/// - Journal state is updated correctly after ack receipt
use forwarder::replay::ReplayEngine;
use forwarder::storage::journal::Journal;
use tempfile::NamedTempFile;

fn make_journal() -> (Journal, NamedTempFile) {
    let f = NamedTempFile::new().unwrap();
    let j = Journal::open(f.path()).unwrap();
    (j, f)
}

// ---------------------------------------------------------------------------
// Cursor resume
// ---------------------------------------------------------------------------

/// Test: replay engine returns events starting after the ack cursor.
#[test]
fn replay_starts_after_ack_cursor() {
    let (mut j, _f) = make_journal();
    j.ensure_stream_state("192.168.2.10", 1).unwrap();

    // Insert 5 events
    for i in 1..=5 {
        let seq = j.next_seq("192.168.2.10").unwrap();
        j.insert_event("192.168.2.10", 1, seq, None, "line", "RAW")
            .unwrap();
        assert_eq!(seq, i);
    }

    // Ack through seq 3
    j.update_ack_cursor("192.168.2.10", 1, 3).unwrap();

    // Replay should return events 4 and 5
    let engine = ReplayEngine::new();
    let result = engine.pending_events(&j, "192.168.2.10").unwrap();

    assert_eq!(result.len(), 1, "one epoch replay batch expected");
    let batch = &result[0];
    assert_eq!(batch.stream_epoch, 1);
    assert_eq!(batch.events.len(), 2, "events 4 and 5 should be pending");
    assert_eq!(batch.events[0].seq, 4);
    assert_eq!(batch.events[1].seq, 5);
}

/// Test: no replay events when all events are acked.
#[test]
fn replay_returns_empty_when_fully_acked() {
    let (mut j, _f) = make_journal();
    j.ensure_stream_state("192.168.2.20", 1).unwrap();

    for _ in 1..=3 {
        let seq = j.next_seq("192.168.2.20").unwrap();
        j.insert_event("192.168.2.20", 1, seq, None, "line", "RAW")
            .unwrap();
    }

    // Ack through seq 3 (all events acked)
    j.update_ack_cursor("192.168.2.20", 1, 3).unwrap();

    let engine = ReplayEngine::new();
    let result = engine.pending_events(&j, "192.168.2.20").unwrap();
    let total_events: usize = result.iter().map(|b| b.events.len()).sum();
    assert_eq!(
        total_events, 0,
        "no events should be pending when fully acked"
    );
}

/// Test: replay returns events from multiple epochs when old-epoch events are unacked.
#[test]
fn replay_includes_old_epoch_unacked_events() {
    let (mut j, _f) = make_journal();
    j.ensure_stream_state("192.168.2.30", 1).unwrap();

    // Write 2 events in epoch 1
    for _ in 1..=2 {
        let seq = j.next_seq("192.168.2.30").unwrap();
        j.insert_event("192.168.2.30", 1, seq, None, "epoch1-event", "RAW")
            .unwrap();
    }

    // Bump to epoch 2 WITHOUT acking epoch 1
    j.bump_epoch("192.168.2.30", 2).unwrap();

    // Write 2 events in epoch 2
    for _ in 1..=2 {
        let seq = j.next_seq("192.168.2.30").unwrap();
        j.insert_event("192.168.2.30", 2, seq, None, "epoch2-event", "RAW")
            .unwrap();
    }

    let engine = ReplayEngine::new();
    let result = engine.pending_events(&j, "192.168.2.30").unwrap();

    // Should have both epoch 1 and epoch 2 batches
    assert!(!result.is_empty(), "should have pending events");
    let total_events: usize = result.iter().map(|b| b.events.len()).sum();
    assert_eq!(
        total_events, 4,
        "all 4 unacked events (2 epoch1 + 2 epoch2) should be pending"
    );
}

/// Test: after replay and ack, cursor advances correctly.
#[test]
fn replay_cursor_advances_after_ack() {
    let (mut j, _f) = make_journal();
    j.ensure_stream_state("192.168.2.40", 1).unwrap();

    for _ in 1..=3 {
        let seq = j.next_seq("192.168.2.40").unwrap();
        j.insert_event("192.168.2.40", 1, seq, None, "line", "RAW")
            .unwrap();
    }

    // Ack seq 2
    j.update_ack_cursor("192.168.2.40", 1, 2).unwrap();

    // Only seq 3 should be pending
    let engine = ReplayEngine::new();
    let result = engine.pending_events(&j, "192.168.2.40").unwrap();
    let total: usize = result.iter().map(|b| b.events.len()).sum();
    assert_eq!(total, 1);
    assert_eq!(result[0].events[0].seq, 3);

    // Now ack seq 3 too
    j.update_ack_cursor("192.168.2.40", 1, 3).unwrap();

    let result2 = engine.pending_events(&j, "192.168.2.40").unwrap();
    let total2: usize = result2.iter().map(|b| b.events.len()).sum();
    assert_eq!(total2, 0, "nothing pending after full ack");
}

/// Test: ack cursor never regresses to an older epoch/seq tuple.
#[test]
fn ack_cursor_does_not_regress() {
    let (mut j, _f) = make_journal();
    j.ensure_stream_state("192.168.2.50", 1).unwrap();

    // Advance to epoch 2 and ack through seq 5.
    j.update_ack_cursor("192.168.2.50", 2, 5).unwrap();

    // Apply an older cursor update; this must be ignored.
    j.update_ack_cursor("192.168.2.50", 1, 999).unwrap();

    let (epoch, seq) = j.ack_cursor("192.168.2.50").unwrap();
    assert_eq!((epoch, seq), (2, 5));
}
