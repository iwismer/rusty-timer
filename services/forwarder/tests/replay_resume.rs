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
        j.insert_event("192.168.2.10", 1, seq, None, b"line", "RAW")
            .unwrap();
        assert_eq!(seq, i);
    }

    // Ack through seq 3
    j.update_receiver_stream_cursor("test-receiver", "192.168.2.10", 3)
        .unwrap();

    // Replay from the durable ack cursor should return events 4 and 5
    let engine = ReplayEngine::new();
    let acked_seq = j.min_acked_through_seq("192.168.2.10").unwrap();
    let batch = engine
        .read_after(&j, "192.168.2.10", acked_seq, 100)
        .unwrap();

    assert!(batch.gap.is_none());
    assert_eq!(batch.records.len(), 2, "events 4 and 5 should be pending");
    assert_eq!(batch.records[0].seq, 4);
    assert_eq!(batch.records[1].seq, 5);
}

/// Test: no replay events when all events are acked.
#[test]
fn replay_returns_empty_when_fully_acked() {
    let (mut j, _f) = make_journal();
    j.ensure_stream_state("192.168.2.20", 1).unwrap();

    for _ in 1..=3 {
        let seq = j.next_seq("192.168.2.20").unwrap();
        j.insert_event("192.168.2.20", 1, seq, None, b"line", "RAW")
            .unwrap();
    }

    // Ack through seq 3 (all events acked)
    j.update_receiver_stream_cursor("test-receiver", "192.168.2.20", 3)
        .unwrap();

    let engine = ReplayEngine::new();
    let acked_seq = j.min_acked_through_seq("192.168.2.20").unwrap();
    let batch = engine
        .read_after(&j, "192.168.2.20", acked_seq, 100)
        .unwrap();
    assert!(batch.gap.is_none());
    assert_eq!(
        batch.records.len(),
        0,
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
        j.insert_event("192.168.2.30", 1, seq, None, b"epoch1-event", "RAW")
            .unwrap();
    }

    // Bump to epoch 2 WITHOUT acking epoch 1
    j.advance_epoch("192.168.2.30", None).unwrap();

    // Write 2 events in epoch 2
    for _ in 1..=2 {
        let seq = j.next_seq("192.168.2.30").unwrap();
        j.insert_event("192.168.2.30", 2, seq, None, b"epoch2-event", "RAW")
            .unwrap();
    }

    let engine = ReplayEngine::new();
    let acked_seq = j.min_acked_through_seq("192.168.2.30").unwrap();
    let batch = engine
        .read_after(&j, "192.168.2.30", acked_seq, 100)
        .unwrap();

    // The stream-wide cursor spans the epoch bump: all 4 events come back in
    // seq order, carrying their original epochs.
    assert!(batch.gap.is_none());
    assert_eq!(
        batch.records.len(),
        4,
        "all 4 unacked events (2 epoch1 + 2 epoch2) should be pending"
    );
    let epochs: Vec<i64> = batch.records.iter().map(|e| e.stream_epoch).collect();
    assert_eq!(epochs, vec![1, 1, 2, 2]);
    let seqs: Vec<i64> = batch.records.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![1, 2, 3, 4]);
}

/// Test: after replay and ack, cursor advances correctly.
#[test]
fn replay_cursor_advances_after_ack() {
    let (mut j, _f) = make_journal();
    j.ensure_stream_state("192.168.2.40", 1).unwrap();

    for _ in 1..=3 {
        let seq = j.next_seq("192.168.2.40").unwrap();
        j.insert_event("192.168.2.40", 1, seq, None, b"line", "RAW")
            .unwrap();
    }

    // Ack seq 2
    j.update_receiver_stream_cursor("test-receiver", "192.168.2.40", 2)
        .unwrap();

    // Only seq 3 should be pending
    let engine = ReplayEngine::new();
    let acked_seq = j.min_acked_through_seq("192.168.2.40").unwrap();
    let batch = engine
        .read_after(&j, "192.168.2.40", acked_seq, 100)
        .unwrap();
    assert_eq!(batch.records.len(), 1);
    assert_eq!(batch.records[0].seq, 3);

    // Now ack seq 3 too
    j.update_receiver_stream_cursor("test-receiver", "192.168.2.40", 3)
        .unwrap();

    let acked_seq = j.min_acked_through_seq("192.168.2.40").unwrap();
    let batch = engine
        .read_after(&j, "192.168.2.40", acked_seq, 100)
        .unwrap();
    assert_eq!(batch.records.len(), 0, "nothing pending after full ack");
}

/// Test: ack cursor ignores stale (lower) seq updates.
///
/// Under the stream-wide sequence contract, staleness is determined purely by
/// seq — a lower-seq ack must never roll the durable cursor backwards. The
/// reported epoch is derived from the event carrying the acked seq.
#[test]
fn ack_cursor_ignores_stale_lower_seq() {
    let (mut j, _f) = make_journal();
    j.ensure_stream_state("192.168.2.50", 1).unwrap();

    // Write seq 1-5 in epoch 1, bump to epoch 2, write seq 6-10.
    for _ in 1..=5 {
        let seq = j.next_seq("192.168.2.50").unwrap();
        j.insert_event("192.168.2.50", 1, seq, None, b"line", "RAW")
            .unwrap();
    }
    j.advance_epoch("192.168.2.50", None).unwrap();
    for _ in 6..=10 {
        let seq = j.next_seq("192.168.2.50").unwrap();
        j.insert_event("192.168.2.50", 2, seq, None, b"line", "RAW")
            .unwrap();
    }

    // Ack through seq 8 (which lands in epoch 2).
    j.update_receiver_stream_cursor("test-receiver", "192.168.2.50", 8)
        .unwrap();
    assert_eq!(j.min_acked_through_seq("192.168.2.50").unwrap(), 8);

    // Apply a stale, lower-seq cursor update; this must be ignored.
    j.update_receiver_stream_cursor("test-receiver", "192.168.2.50", 3)
        .unwrap();

    let seq = j.min_acked_through_seq("192.168.2.50").unwrap();
    assert_eq!(seq, 8, "stale lower-seq ack must be ignored");
}
