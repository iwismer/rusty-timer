/// Tests for journal pruning after ack cursor update.
///
/// Verifies that `prune_acked` removes acked events from the journal
/// to prevent unbounded growth on long-running SBCs.
use forwarder::storage::journal::{Journal, MAX_PRUNE_BATCH, RetentionContext, RetentionPolicy};
use std::path::PathBuf;

/// Helper: open a Journal backed by a temporary directory on disk.
fn open_temp_journal() -> (Journal, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("journal.db");
    let journal = Journal::open(&path).expect("open journal");
    (journal, dir)
}

/// Helper: open a Journal and keep its database path for direct test setup.
fn open_temp_journal_with_path() -> (Journal, PathBuf, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("journal.db");
    let journal = Journal::open(&path).expect("open journal");
    (journal, path, dir)
}

/// Helper: insert a raw event into the journal for testing.
fn insert_event(journal: &mut Journal, stream_key: &str, epoch: i64, seq: i64) {
    journal
        .insert_event(stream_key, epoch, seq, None, b"aa01,frame", "RAW")
        .expect("insert event");
}

/// Bulk-insert `count` events (seq 1..=count) for a stream in a single
/// transaction with an old `received_unix_ms`, bypassing per-insert fsyncs so
/// large-batch tests stay fast.
fn insert_old_events_bulk(path: &PathBuf, stream_key: &str, epoch: i64, count: i64) {
    let mut conn = rusqlite::Connection::open(path).expect("open sqlite connection");
    let tx = conn.transaction().expect("begin tx");
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO events
                     (stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms)
                 VALUES (?1, ?2, ?3, ?4, 'RAW', NULL, 0)",
            )
            .expect("prepare insert");
        for seq in 1..=count {
            stmt.execute(rusqlite::params![
                stream_key,
                seq,
                epoch,
                b"aa01,frame".as_slice()
            ])
            .expect("bulk insert event");
        }
    }
    tx.commit().expect("commit bulk insert");
}

fn set_received_unix_ms(path: &PathBuf, stream_key: &str, seq: i64, received_unix_ms: i64) {
    let conn = rusqlite::Connection::open(path).expect("open sqlite connection");
    conn.execute(
        "UPDATE events SET received_unix_ms = ?1 WHERE stream_id = ?2 AND seq = ?3",
        rusqlite::params![received_unix_ms, stream_key, seq],
    )
    .expect("set received_unix_ms");
}

fn remaining_seqs(journal: &Journal, stream_key: &str) -> Vec<i64> {
    journal
        .read_events_after(stream_key, 0, usize::MAX)
        .expect("events")
        .into_iter()
        .map(|event| event.seq)
        .collect()
}

fn test_policy(emergency_max_rows: i64) -> RetentionPolicy {
    RetentionPolicy {
        min_retention_ms: 7 * 24 * 60 * 60 * 1000,
        max_retention_ms: 30 * 24 * 60 * 60 * 1000,
        emergency_free_disk_bytes: 1_000_000_000,
        emergency_max_rows,
    }
}

fn retention_context(now_unix_ms: i64) -> RetentionContext {
    RetentionContext {
        now_unix_ms,
        free_disk_bytes: 2_000_000_000,
    }
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
        .update_receiver_stream_cursor("test-receiver", stream_key, 3)
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
        .read_events_after(stream_key, 0, usize::MAX)
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
        .update_receiver_stream_cursor("test-receiver", stream_key, 10)
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

    // Insert 3 events in epoch 1 (stream-wide seq 1-3)
    for seq in 1..=3 {
        insert_event(&mut journal, stream_key, 1, seq);
    }

    // Bump to epoch 2 and insert 2 more events. Seq is stream-wide and does
    // not reset, so epoch 2 events are seq 4 and 5.
    journal.advance_epoch(stream_key, None).expect("bump epoch");
    for seq in 4..=5 {
        insert_event(&mut journal, stream_key, 2, seq);
    }

    assert_eq!(journal.event_count(stream_key).unwrap(), 5);

    // Ack through seq 4 — this covers all of epoch 1 (seq 1-3) and seq 4 of
    // epoch 2.
    journal
        .update_receiver_stream_cursor("test-receiver", stream_key, 4)
        .expect("update ack cursor");

    let deleted = journal.prune_acked(stream_key, 500).expect("prune acked");
    assert_eq!(
        deleted, 4,
        "should have pruned 3 epoch-1 events + 1 epoch-2 event"
    );

    // Only epoch 2, seq 5 should remain
    assert_eq!(journal.event_count(stream_key).unwrap(), 1);
    let remaining = journal
        .read_events_after(stream_key, 0, usize::MAX)
        .expect("unacked events");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].seq, 5);
    assert_eq!(remaining[0].stream_epoch, 2);
}

/// Test: full ack-then-prune cycle — simulates the runtime pattern.
///
/// This mirrors what the P2P loop does: update_ack_cursor then prune_acked.
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

    // Simulate: ack cursor update followed by prune (as done in main.rs P2P loop)
    journal
        .update_receiver_stream_cursor("test-receiver", stream_key, 10)
        .expect("update ack cursor");
    let deleted = journal.prune_acked(stream_key, 500).expect("prune acked");

    assert_eq!(deleted, 10, "all 10 events should be pruned after full ack");
    assert_eq!(
        journal.total_event_count().unwrap(),
        0,
        "journal should be empty after full ack-prune cycle"
    );
}

/// Regression test: after a full ack + prune empties the journal, `next_seq`
/// must continue above the pruned range instead of restarting at 1 and
/// reusing already-acked seqs.
#[test]
fn next_seq_continues_above_pruned_range() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "192.168.1.105";
    let epoch = 1i64;

    journal
        .ensure_stream_state(stream_key, epoch)
        .expect("ensure stream state");

    // Insert and fully ack/prune events seq 1-5.
    for _ in 0..5 {
        let seq = journal.next_seq(stream_key).expect("next seq");
        insert_event(&mut journal, stream_key, epoch, seq);
    }
    journal
        .update_receiver_stream_cursor("test-receiver", stream_key, 5)
        .expect("update ack cursor");
    let deleted = journal.prune_acked(stream_key, 500).expect("prune acked");
    assert_eq!(deleted, 5, "all 5 events pruned");
    assert_eq!(
        journal.event_count(stream_key).unwrap(),
        0,
        "journal empty after prune"
    );

    // next_seq must NOT restart at 1 — it must continue above the pruned range.
    let next = journal.next_seq(stream_key).expect("next seq after prune");
    assert_eq!(
        next, 6,
        "next_seq must continue above the pruned/acked high-water, not reuse seqs"
    );

    // Insert at seq 6, ack/prune again, and confirm it keeps climbing.
    insert_event(&mut journal, stream_key, epoch, next);
    journal
        .update_receiver_stream_cursor("test-receiver", stream_key, next)
        .expect("update ack cursor");
    journal.prune_acked(stream_key, 500).expect("prune acked");
    assert_eq!(
        journal.next_seq(stream_key).expect("next seq"),
        7,
        "next_seq continues climbing after repeated ack/prune cycles"
    );
}

#[test]
fn rule1_age_floor_protects() {
    let (mut journal, path, _dir) = open_temp_journal_with_path();
    let stream_key = "192.168.1.110";
    let epoch = 1i64;
    let now = 50 * 24 * 60 * 60 * 1000;

    journal.ensure_stream_state(stream_key, epoch).unwrap();
    for seq in 1..=2 {
        insert_event(&mut journal, stream_key, epoch, seq);
    }
    set_received_unix_ms(&path, stream_key, 1, now - 8 * 24 * 60 * 60 * 1000);
    set_received_unix_ms(&path, stream_key, 2, now - 6 * 24 * 60 * 60 * 1000);
    journal
        .update_receiver_stream_cursor("test-receiver", stream_key, 2)
        .unwrap();

    let stats = journal
        .prune_retention(&test_policy(1_000_000), retention_context(now))
        .expect("prune retention");

    assert_eq!(stats.acked_deleted, 1);
    assert_eq!(remaining_seqs(&journal, stream_key), vec![2]);
    let retention = journal.retention_state(stream_key).unwrap();
    assert_eq!(retention.earliest_available_seq, 2);
    assert_eq!(retention.forced_gap_count, 0);
}

#[test]
fn rule2_prunes_acked_old() {
    let (mut journal, path, _dir) = open_temp_journal_with_path();
    let stream_key = "192.168.1.111";
    let epoch = 1i64;
    let now = 10 * 24 * 60 * 60 * 1000;

    journal.ensure_stream_state(stream_key, epoch).unwrap();
    for seq in 1..=3 {
        insert_event(&mut journal, stream_key, epoch, seq);
        set_received_unix_ms(&path, stream_key, seq, 0);
    }
    journal
        .update_receiver_stream_cursor("test-receiver", stream_key, 2)
        .unwrap();

    let stats = journal
        .prune_retention(&test_policy(1_000_000), retention_context(now))
        .expect("prune retention");

    assert_eq!(stats.acked_deleted, 2);
    assert_eq!(remaining_seqs(&journal, stream_key), vec![3]);
    let retention = journal.retention_state(stream_key).unwrap();
    assert_eq!(retention.earliest_available_seq, 3);
    assert_eq!(retention.forced_gap_count, 0);
}

#[test]
fn rule3_hard_cap_prunes_unacked() {
    let (mut journal, path, _dir) = open_temp_journal_with_path();
    let stream_key = "192.168.1.112";
    let epoch = 1i64;
    let day = 24 * 60 * 60 * 1000;
    let now = 40 * day;

    journal.ensure_stream_state(stream_key, epoch).unwrap();
    for seq in 1..=2 {
        insert_event(&mut journal, stream_key, epoch, seq);
    }
    set_received_unix_ms(&path, stream_key, 1, now - 31 * day);
    set_received_unix_ms(&path, stream_key, 2, now - 10 * day);

    let stats = journal
        .prune_retention(&test_policy(1_000_000), retention_context(now))
        .expect("prune retention");

    assert_eq!(stats.hard_cap_deleted, 1);
    assert_eq!(remaining_seqs(&journal, stream_key), vec![2]);
    let retention = journal.retention_state(stream_key).unwrap();
    assert_eq!(retention.earliest_available_seq, 2);
    assert_eq!(retention.forced_gap_count, 1);
}

#[test]
fn rule4_storage_emergency() {
    let (mut journal, path, _dir) = open_temp_journal_with_path();
    let stream_key = "192.168.1.113";
    let epoch = 1i64;
    let day = 24 * 60 * 60 * 1000;
    let now = 20 * day;

    journal.ensure_stream_state(stream_key, epoch).unwrap();
    for seq in 1..=5 {
        insert_event(&mut journal, stream_key, epoch, seq);
        set_received_unix_ms(&path, stream_key, seq, now - 10 * day);
    }

    let stats = journal
        .prune_retention(&test_policy(3), retention_context(now))
        .expect("prune retention");

    assert_eq!(stats.emergency_deleted, 2);
    assert_eq!(remaining_seqs(&journal, stream_key), vec![3, 4, 5]);
    let retention = journal.retention_state(stream_key).unwrap();
    assert_eq!(retention.earliest_available_seq, 3);
    assert_eq!(retention.forced_gap_count, 2);
}

/// Regression: a forced (hard-cap) prune must never delete a non-contiguous
/// per-stream row. Here seq 2 is old (beyond the hard cap) while seq 1 is new
/// (newer low seq, older high seq). Deleting seq 2 while seq 1 remains would
/// drop the live high-water and let `next_seq` reuse the already-issued seq 2.
#[test]
fn forced_prune_preserves_high_water_with_newer_low_seq() {
    let (mut journal, path, _dir) = open_temp_journal_with_path();
    let stream_key = "192.168.1.120";
    let epoch = 1i64;
    let day = 24 * 60 * 60 * 1000;
    let now = 40 * day;

    journal.ensure_stream_state(stream_key, epoch).unwrap();
    for seq in 1..=2 {
        insert_event(&mut journal, stream_key, epoch, seq);
    }
    // seq 1 is recent (inside max retention); seq 2 is old (beyond hard cap).
    set_received_unix_ms(&path, stream_key, 1, now - day);
    set_received_unix_ms(&path, stream_key, 2, now - 31 * day);

    let stats = journal
        .prune_retention(&test_policy(1_000_000), retention_context(now))
        .expect("prune retention");

    // The old high seq 2 must NOT be deleted while the newer low seq 1 remains.
    assert_eq!(
        stats.hard_cap_deleted, 0,
        "non-prefix forced deletion of seq 2 must not occur while seq 1 remains"
    );
    assert_eq!(remaining_seqs(&journal, stream_key), vec![1, 2]);
    let retention = journal.retention_state(stream_key).unwrap();
    assert_eq!(retention.earliest_available_seq, 1);
    assert_eq!(
        journal.next_seq(stream_key).unwrap(),
        3,
        "next_seq must not reuse the already-issued seq 2"
    );

    // Once the low seq also ages out, the full contiguous prefix is prunable
    // and the high-water is still preserved (no seq reuse).
    set_received_unix_ms(&path, stream_key, 1, now - 31 * day);
    let stats = journal
        .prune_retention(&test_policy(1_000_000), retention_context(now))
        .expect("prune retention again");
    assert_eq!(stats.hard_cap_deleted, 2);
    assert!(remaining_seqs(&journal, stream_key).is_empty());
    assert_eq!(
        journal.next_seq(stream_key).unwrap(),
        3,
        "next_seq stays monotonic after the full prefix is pruned"
    );
}

/// A single `prune_retention` pass is bounded by `MAX_PRUNE_BATCH` per category,
/// so it cannot collect an unbounded candidate set or delete the whole journal
/// in one transaction. Remaining rows are pruned on subsequent passes.
#[test]
fn prune_retention_bounds_batch_size() {
    let (mut journal, path, _dir) = open_temp_journal_with_path();
    let stream_key = "192.168.1.121";
    let epoch = 1i64;
    let now = 10 * 24 * 60 * 60 * 1000;
    let total = MAX_PRUNE_BATCH + 5;

    journal.ensure_stream_state(stream_key, epoch).unwrap();
    insert_old_events_bulk(&path, stream_key, epoch, total);
    journal
        .update_receiver_stream_cursor("test-receiver", stream_key, total)
        .unwrap();
    assert_eq!(journal.event_count(stream_key).unwrap(), total);

    let stats = journal
        .prune_retention(&test_policy(1_000_000), retention_context(now))
        .expect("prune retention");
    assert_eq!(
        stats.acked_deleted, MAX_PRUNE_BATCH,
        "a single pass deletes at most MAX_PRUNE_BATCH acked rows"
    );
    assert_eq!(journal.event_count(stream_key).unwrap(), 5);

    // A second pass drains the remainder.
    let stats = journal
        .prune_retention(&test_policy(1_000_000), retention_context(now))
        .expect("prune retention second pass");
    assert_eq!(stats.acked_deleted, 5);
    assert_eq!(journal.event_count(stream_key).unwrap(), 0);
}

#[test]
fn manual_clear_keeps_seq_monotonic() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "192.168.1.114";

    journal.ensure_stream_state(stream_key, 1).unwrap();
    for frame in [b"a".as_slice(), b"b".as_slice(), b"c".as_slice()] {
        journal.append_read(stream_key, None, frame, "RAW").unwrap();
    }

    journal.clear_stream(stream_key).expect("clear stream");
    let (epoch, seq) = journal
        .append_read(stream_key, None, b"after-clear", "RAW")
        .expect("append after clear");

    assert_eq!(epoch, 2);
    assert_eq!(seq, 4);
}

#[test]
fn clear_bumps_epoch() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "192.168.1.115";

    journal.ensure_stream_state(stream_key, 1).unwrap();
    journal
        .append_read(stream_key, None, b"before-clear", "RAW")
        .unwrap();
    journal.clear_stream(stream_key).expect("clear stream");

    let (epoch, next_seq) = journal.current_epoch_and_next_seq(stream_key).unwrap();
    assert_eq!(epoch, 2);
    assert_eq!(next_seq, 2);
    assert_eq!(journal.event_count(stream_key).unwrap(), 0);
}
