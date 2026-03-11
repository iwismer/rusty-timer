/// Tests for journal pruning after ack cursor update.
///
/// Verifies that `prune_acked` removes acked events from the journal
/// to prevent unbounded growth on long-running SBCs.
use forwarder::storage::journal::Journal;

/// Helper: open a Journal backed by a temporary directory on disk.
fn open_temp_journal() -> (Journal, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("journal.db");
    let journal = Journal::open(&path).expect("open journal");
    (journal, dir)
}

/// Helper: insert a raw event into the journal for testing.
fn insert_event(journal: &mut Journal, stream_key: &str, epoch: i64, seq: i64) {
    journal
        .insert_event(stream_key, epoch, seq, None, b"aa01,frame", "RAW")
        .expect("insert event");
}

// ---------------------------------------------------------------------------
// prune_acked removes acked events
// ---------------------------------------------------------------------------

/// Test: after updating ack cursor, prune_acked removes the acked events.
#[test]
fn prune_acked_removes_acked_events() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "192.168.1.100";
    let epoch = 1i64;

    journal
        .ensure_stream_state(stream_key, epoch)
        .expect("ensure stream state");

    // Insert 5 events in epoch 1
    for seq in 1..=5 {
        insert_event(&mut journal, stream_key, epoch, seq);
    }
    assert_eq!(
        journal.event_count(stream_key).unwrap(),
        5,
        "should have 5 events before pruning"
    );

    // Ack through seq 3
    journal
        .update_ack_cursor(stream_key, epoch, 3)
        .expect("update ack cursor");

    // Prune up to 500 acked events
    let deleted = journal.prune_acked(stream_key, 500).expect("prune acked");
    assert_eq!(deleted, 3, "should have pruned 3 acked events (seq 1-3)");

    // Only 2 unacked events (seq 4 and 5) should remain
    assert_eq!(
        journal.event_count(stream_key).unwrap(),
        2,
        "should have 2 events remaining after pruning"
    );

    // Verify that the remaining events are seq 4 and 5
    let remaining = journal
        .unacked_events(stream_key, epoch, 0)
        .expect("unacked events");
    assert_eq!(remaining.len(), 2);
    assert_eq!(remaining[0].seq, 4);
    assert_eq!(remaining[1].seq, 5);
}

/// Test: prune_acked respects the limit parameter.
#[test]
fn prune_acked_respects_limit() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "192.168.1.101";
    let epoch = 1i64;

    journal
        .ensure_stream_state(stream_key, epoch)
        .expect("ensure stream state");

    // Insert 10 events
    for seq in 1..=10 {
        insert_event(&mut journal, stream_key, epoch, seq);
    }

    // Ack all 10
    journal
        .update_ack_cursor(stream_key, epoch, 10)
        .expect("update ack cursor");

    // Prune with a limit of 3
    let deleted = journal.prune_acked(stream_key, 3).expect("prune acked");
    assert_eq!(
        deleted, 3,
        "should have pruned exactly 3 events (limit respected)"
    );

    // 7 events should remain
    assert_eq!(
        journal.event_count(stream_key).unwrap(),
        7,
        "should have 7 events remaining"
    );
}

/// Test: prune_acked with no acked events deletes nothing.
#[test]
fn prune_acked_with_no_acked_events_deletes_nothing() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "192.168.1.102";
    let epoch = 1i64;

    journal
        .ensure_stream_state(stream_key, epoch)
        .expect("ensure stream state");

    // Insert 5 events but don't ack any (cursor starts at 0)
    for seq in 1..=5 {
        insert_event(&mut journal, stream_key, epoch, seq);
    }

    // Prune without any acks — should delete nothing
    let deleted = journal.prune_acked(stream_key, 500).expect("prune acked");
    assert_eq!(
        deleted, 0,
        "should not delete anything when ack cursor is at 0"
    );

    assert_eq!(
        journal.event_count(stream_key).unwrap(),
        5,
        "all events should remain"
    );
}

/// Test: prune_acked also removes events from older epochs.
#[test]
fn prune_acked_removes_older_epoch_events() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "192.168.1.103";

    journal
        .ensure_stream_state(stream_key, 1)
        .expect("ensure stream state");

    // Insert 3 events in epoch 1
    for seq in 1..=3 {
        insert_event(&mut journal, stream_key, 1, seq);
    }

    // Bump to epoch 2 and insert 2 more events
    journal.bump_epoch(stream_key, 2).expect("bump epoch");
    for seq in 1..=2 {
        insert_event(&mut journal, stream_key, 2, seq);
    }

    assert_eq!(journal.event_count(stream_key).unwrap(), 5);

    // Ack through epoch 2, seq 1 — this covers all of epoch 1 and seq 1 of epoch 2
    journal
        .update_ack_cursor(stream_key, 2, 1)
        .expect("update ack cursor");

    let deleted = journal.prune_acked(stream_key, 500).expect("prune acked");
    assert_eq!(
        deleted, 4,
        "should have pruned 3 epoch-1 events + 1 epoch-2 event"
    );

    // Only epoch 2, seq 2 should remain
    assert_eq!(journal.event_count(stream_key).unwrap(), 1);
    let remaining = journal
        .unacked_events(stream_key, 2, 0)
        .expect("unacked events");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].seq, 2);
    assert_eq!(remaining[0].stream_epoch, 2);
}

/// Test: full ack-then-prune cycle — simulates the runtime pattern.
///
/// This mirrors what the uplink loop does: update_ack_cursor then prune_acked.
#[test]
fn ack_then_prune_cycle_clears_journal() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "192.168.1.104";
    let epoch = 1i64;

    journal
        .ensure_stream_state(stream_key, epoch)
        .expect("ensure stream state");

    // Insert 10 events
    for seq in 1..=10 {
        insert_event(&mut journal, stream_key, epoch, seq);
    }

    // Simulate: ack cursor update followed by prune (as done in main.rs uplink loop)
    journal
        .update_ack_cursor(stream_key, epoch, 10)
        .expect("update ack cursor");
    let deleted = journal.prune_acked(stream_key, 500).expect("prune acked");

    assert_eq!(deleted, 10, "all 10 events should be pruned after full ack");
    assert_eq!(
        journal.total_event_count().unwrap(),
        0,
        "journal should be empty after full ack-prune cycle"
    );
}
