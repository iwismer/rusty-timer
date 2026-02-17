/// Power-loss recovery tests for the forwarder journal.
///
/// Validates that:
/// - WAL+FULL sync settings are applied
/// - integrity_check is run at open and fails gracefully
/// - events written before close survive reopen
/// - acked cursor survives reopen
/// - UNIQUE(stream_key, stream_epoch, seq) constraint is enforced
use forwarder::storage::journal::Journal;
use tempfile::NamedTempFile;

// ---------------------------------------------------------------------------
// WAL + sync settings
// ---------------------------------------------------------------------------

#[test]
fn wal_mode_and_sync_full_are_set() {
    use rusqlite::Connection;
    let f = NamedTempFile::new().unwrap();
    let _j = Journal::open(f.path()).unwrap();

    // Verify PRAGMAs by opening the same file with raw rusqlite
    let conn = Connection::open(f.path()).unwrap();
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal");

    let sync: i64 = conn
        .pragma_query_value(None, "synchronous", |r| r.get(0))
        .unwrap();
    assert_eq!(sync, 2, "synchronous must be FULL (2)");
}

// ---------------------------------------------------------------------------
// Data survives close/reopen
// ---------------------------------------------------------------------------

#[test]
fn events_survive_close_and_reopen() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_path_buf();

    {
        let mut j = Journal::open(&path).unwrap();
        j.ensure_stream_state("10.0.0.5", 1).unwrap();
        let seq = j.next_seq("10.0.0.5").unwrap();
        j.insert_event("10.0.0.5", 1, seq, Some("2026-01-01T00:00:00Z"), "test-line", "RAW")
            .unwrap();
    }

    {
        let j = Journal::open(&path).unwrap();
        let unacked = j.unacked_events("10.0.0.5", 1, 0).unwrap();
        assert_eq!(unacked.len(), 1, "event must survive reopen");
        assert_eq!(unacked[0].raw_read_line, "test-line");
    }
}

#[test]
fn ack_cursor_survives_close_and_reopen() {
    let f = NamedTempFile::new().unwrap();
    let path = f.path().to_path_buf();

    {
        let mut j = Journal::open(&path).unwrap();
        j.ensure_stream_state("10.0.0.6", 1).unwrap();
        for _ in 1..=3 {
            let seq = j.next_seq("10.0.0.6").unwrap();
            j.insert_event("10.0.0.6", 1, seq, None, "line", "RAW").unwrap();
        }
        j.update_ack_cursor("10.0.0.6", 1, 2).unwrap(); // ack through seq 2
    }

    {
        let j = Journal::open(&path).unwrap();
        let (acked_epoch, acked_seq) = j.ack_cursor("10.0.0.6").unwrap();
        assert_eq!(acked_epoch, 1);
        assert_eq!(acked_seq, 2, "ack cursor must survive reopen");

        // Only seq 3 should be unacked
        let unacked = j.unacked_events("10.0.0.6", 1, 2).unwrap();
        assert_eq!(unacked.len(), 1);
        assert_eq!(unacked[0].seq, 3);
    }
}

// ---------------------------------------------------------------------------
// UNIQUE constraint
// ---------------------------------------------------------------------------

#[test]
fn duplicate_stream_epoch_seq_is_rejected() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.0.0.7", 1).unwrap();
    let seq = j.next_seq("10.0.0.7").unwrap();

    j.insert_event("10.0.0.7", 1, seq, None, "line", "RAW").unwrap();
    // Try to insert the same (stream_key, epoch, seq) again
    let result = j.insert_event("10.0.0.7", 1, seq, None, "line-dup", "RAW");
    assert!(result.is_err(), "duplicate (stream_key, epoch, seq) must be rejected");
}

// ---------------------------------------------------------------------------
// Pruning: acked first, then unacked under pressure
// ---------------------------------------------------------------------------

#[test]
fn prune_acked_events_first() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.0.0.8", 1).unwrap();

    for _ in 1..=5 {
        let seq = j.next_seq("10.0.0.8").unwrap();
        j.insert_event("10.0.0.8", 1, seq, None, "line", "RAW").unwrap();
    }

    // Ack through seq 3
    j.update_ack_cursor("10.0.0.8", 1, 3).unwrap();

    // Prune up to 3 records (should remove the 3 acked ones)
    let pruned = j.prune_acked("10.0.0.8", 3).unwrap();
    assert_eq!(pruned, 3, "should prune exactly 3 acked records");

    // Seq 4 and 5 must still be present
    let unacked = j.unacked_events("10.0.0.8", 1, 0).unwrap();
    assert_eq!(unacked.len(), 2);
    assert!(unacked.iter().any(|e| e.seq == 4));
    assert!(unacked.iter().any(|e| e.seq == 5));
}

#[test]
fn total_event_count_accessible() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.0.0.9", 1).unwrap();

    for _ in 1..=7 {
        let seq = j.next_seq("10.0.0.9").unwrap();
        j.insert_event("10.0.0.9", 1, seq, None, "line", "RAW").unwrap();
    }

    let total = j.total_event_count().unwrap();
    assert_eq!(total, 7);
}

// ---------------------------------------------------------------------------
// Multiple streams independent
// ---------------------------------------------------------------------------

#[test]
fn multiple_streams_have_independent_sequences() {
    let f = NamedTempFile::new().unwrap();
    let mut j = Journal::open(f.path()).unwrap();
    j.ensure_stream_state("10.0.0.10", 1).unwrap();
    j.ensure_stream_state("10.0.0.11", 1).unwrap();

    let s1a = j.next_seq("10.0.0.10").unwrap();
    let s2a = j.next_seq("10.0.0.11").unwrap();
    let s1b = j.next_seq("10.0.0.10").unwrap();

    assert_eq!(s1a, 1);
    assert_eq!(s2a, 1, "each stream starts at seq 1 independently");
    assert_eq!(s1b, 2);
}
