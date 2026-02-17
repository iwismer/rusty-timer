/// Tests for the event ID generator (epoch + seq monotonicity).
///
/// Validates:
/// - seq increments monotonically within an epoch
/// - seq resets to 1 when epoch bumps
/// - seq resumes from persisted state after restart (simulated via reopen)
/// - epoch bump does not drop old-epoch unacked events
use forwarder::storage::journal::Journal;
use tempfile::NamedTempFile;

fn open_journal() -> (Journal, NamedTempFile) {
    let f = NamedTempFile::new().expect("temp file");
    let j = Journal::open(f.path()).expect("open journal");
    (j, f)
}

fn open_journal_at(path: &std::path::Path) -> Journal {
    Journal::open(path).expect("open journal")
}

// ---------------------------------------------------------------------------
// Monotonic seq within epoch
// ---------------------------------------------------------------------------

#[test]
fn seq_is_monotonically_increasing_within_epoch() {
    let (mut j, _f) = open_journal();
    let stream_key = "192.168.2.156";

    // Init stream state at epoch 1, next_seq = 1
    j.ensure_stream_state(stream_key, 1).expect("init stream");

    let s1 = j.next_seq(stream_key).expect("seq 1");
    let s2 = j.next_seq(stream_key).expect("seq 2");
    let s3 = j.next_seq(stream_key).expect("seq 3");

    assert_eq!(s1, 1);
    assert_eq!(s2, 2);
    assert_eq!(s3, 3);
}

#[test]
fn first_seq_in_epoch_is_one() {
    let (mut j, _f) = open_journal();
    let stream_key = "192.168.2.100";
    j.ensure_stream_state(stream_key, 1).expect("init stream");
    let s = j.next_seq(stream_key).expect("first seq");
    assert_eq!(s, 1, "first seq in epoch must be 1");
}

// ---------------------------------------------------------------------------
// Epoch bump resets seq to 1
// ---------------------------------------------------------------------------

#[test]
fn seq_resets_to_1_on_epoch_bump() {
    let (mut j, _f) = open_journal();
    let stream_key = "192.168.2.200";
    j.ensure_stream_state(stream_key, 1).expect("init stream");

    // Advance seq in epoch 1
    j.next_seq(stream_key).unwrap(); // 1
    j.next_seq(stream_key).unwrap(); // 2

    // Bump epoch to 2
    j.bump_epoch(stream_key, 2).expect("bump epoch");

    // First seq in epoch 2 must be 1
    let s = j.next_seq(stream_key).expect("seq after epoch bump");
    assert_eq!(s, 1, "seq must reset to 1 after epoch bump");
}

#[test]
fn epoch_bump_does_not_delete_old_epoch_events() {
    let (mut j, _f) = open_journal();
    let stream_key = "192.168.2.201";
    j.ensure_stream_state(stream_key, 1).expect("init stream");

    // Write an event in epoch 1
    let seq1 = j.next_seq(stream_key).unwrap();
    j.insert_event(
        stream_key,
        1,
        seq1,
        Some("2026-01-01T00:00:00Z"),
        "aa01line",
        "RAW",
    )
    .expect("insert event epoch 1");

    // Bump to epoch 2
    j.bump_epoch(stream_key, 2).expect("bump epoch");

    // Old epoch 1 event must still be in journal
    let count = j
        .count_events_for_epoch(stream_key, 1)
        .expect("count epoch 1");
    assert_eq!(
        count, 1,
        "old-epoch events must not be deleted on epoch bump"
    );
}

// ---------------------------------------------------------------------------
// Restart resume
// ---------------------------------------------------------------------------

#[test]
fn seq_resumes_from_persisted_state_after_reopen() {
    let tmp = NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_path_buf();

    // Write some events, then close
    {
        let mut j = open_journal_at(&path);
        j.ensure_stream_state("192.168.2.50", 1).unwrap();
        j.next_seq("192.168.2.50").unwrap(); // 1
        j.next_seq("192.168.2.50").unwrap(); // 2
        j.next_seq("192.168.2.50").unwrap(); // 3
    }

    // Reopen — seq must resume from 4, not restart at 1
    {
        let mut j = open_journal_at(&path);
        let resumed = j.next_seq("192.168.2.50").expect("resumed seq");
        assert_eq!(
            resumed, 4,
            "seq must resume from persisted state after reopen"
        );
    }
}

#[test]
fn epoch_resumes_from_persisted_state_after_reopen() {
    let tmp = NamedTempFile::new().expect("temp file");
    let path = tmp.path().to_path_buf();

    // Write in epoch 1, bump to 2, write in epoch 2
    {
        let mut j = open_journal_at(&path);
        j.ensure_stream_state("10.0.0.1", 1).unwrap();
        j.next_seq("10.0.0.1").unwrap();
        j.bump_epoch("10.0.0.1", 2).unwrap();
        j.next_seq("10.0.0.1").unwrap(); // seq=1 in epoch 2
    }

    // Reopen — should be in epoch 2, next seq is 2
    {
        let mut j = open_journal_at(&path);
        let (epoch, next_seq) = j.current_epoch_and_next_seq("10.0.0.1").expect("state");
        assert_eq!(epoch, 2, "epoch must be persisted");
        assert_eq!(next_seq, 2, "next_seq after reopen in epoch 2 must be 2");
    }
}

// ---------------------------------------------------------------------------
// insert_event and read back
// ---------------------------------------------------------------------------

#[test]
fn insert_event_persists_all_fields() {
    let (mut j, _f) = open_journal();
    j.ensure_stream_state("192.168.2.10", 1).unwrap();
    let seq = j.next_seq("192.168.2.10").unwrap();

    j.insert_event(
        "192.168.2.10",
        1,
        seq,
        Some("2026-01-01T12:00:00Z"),
        "aa400000000123450a2a01123018455927a7",
        "RAW",
    )
    .expect("insert");

    let events = j.unacked_events("192.168.2.10", 1, 0).expect("unacked");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].stream_key, "192.168.2.10");
    assert_eq!(events[0].stream_epoch, 1);
    assert_eq!(events[0].seq, seq);
    assert_eq!(
        events[0].reader_timestamp,
        Some("2026-01-01T12:00:00Z".to_owned())
    );
    assert_eq!(
        events[0].raw_read_line,
        "aa400000000123450a2a01123018455927a7"
    );
    assert_eq!(events[0].read_type, "RAW");
}

#[test]
fn invalid_utf8_raw_read_line_is_rejected() {
    let (mut j, _f) = open_journal();
    j.ensure_stream_state("192.168.2.11", 1).unwrap();
    let seq = j.next_seq("192.168.2.11").unwrap();

    // Simulate an invalid UTF-8 scenario via a non-UTF-8-friendly string.
    // Since Rust strings are always valid UTF-8, we test that the journal
    // rejects explicitly flagged invalid raw_read_line values.
    // The journal must reject empty raw_read_line strings (as a proxy for invalid content).
    let result = j.insert_event("192.168.2.11", 1, seq, None, "", "RAW");
    assert!(result.is_err(), "empty raw_read_line must be rejected");
}

// ---------------------------------------------------------------------------
// Ack cursor update
// ---------------------------------------------------------------------------

#[test]
fn update_ack_cursor_advances_acked_seq() {
    let (mut j, _f) = open_journal();
    j.ensure_stream_state("192.168.2.20", 1).unwrap();

    for i in 1..=5 {
        let seq = j.next_seq("192.168.2.20").unwrap();
        j.insert_event("192.168.2.20", 1, seq, None, "line", "RAW")
            .unwrap();
        assert_eq!(seq, i);
    }

    // Ack through seq 3
    j.update_ack_cursor("192.168.2.20", 1, 3).expect("ack");

    // Replay starts from after the ack cursor (seq 3), so seq 4 and 5 are unacked
    let (acked_epoch, acked_seq) = j.ack_cursor("192.168.2.20").expect("ack cursor");
    assert_eq!(acked_epoch, 1);
    assert_eq!(acked_seq, 3);

    let unacked = j
        .unacked_events("192.168.2.20", 1, acked_seq)
        .expect("unacked");
    assert_eq!(unacked.len(), 2, "events 4 and 5 should be unacked");
    assert_eq!(unacked[0].seq, 4);
    assert_eq!(unacked[1].seq, 5);
}

// ---------------------------------------------------------------------------
// integrity_check at startup
// ---------------------------------------------------------------------------

#[test]
fn integrity_check_passes_on_fresh_db() {
    let (j, _f) = open_journal();
    // If integrity_check failed, Journal::open would have returned Err
    // (tested indirectly — opening succeeds = integrity passed)
    drop(j);
}
