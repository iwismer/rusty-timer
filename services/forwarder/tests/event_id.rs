/// Tests for the event ID generator (epoch + stream-wide seq monotonicity).
///
/// Validates the new stream-wide sequence contract:
/// - seq increments monotonically as events are inserted
/// - seq is stream-wide and does NOT reset across an epoch bump
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

    // next_seq is a pure read of durable high-water evidence, so it only
    // advances once the previously-issued seq has actually been persisted.
    let s1 = j.next_seq(stream_key).expect("seq 1");
    j.insert_event(stream_key, 1, s1, None, b"line", "RAW")
        .expect("insert 1");
    let s2 = j.next_seq(stream_key).expect("seq 2");
    j.insert_event(stream_key, 1, s2, None, b"line", "RAW")
        .expect("insert 2");
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
// Epoch bump does NOT reset seq (stream-wide sequence)
// ---------------------------------------------------------------------------

#[test]
fn seq_continues_across_epoch_bump() {
    let (mut j, _f) = open_journal();
    let stream_key = "192.168.2.200";
    j.ensure_stream_state(stream_key, 1).expect("init stream");

    // Write two events in epoch 1 (seq 1 and 2)
    let s1 = j.next_seq(stream_key).unwrap();
    j.insert_event(stream_key, 1, s1, None, b"line", "RAW")
        .unwrap();
    let s2 = j.next_seq(stream_key).unwrap();
    j.insert_event(stream_key, 1, s2, None, b"line", "RAW")
        .unwrap();
    assert_eq!((s1, s2), (1, 2));

    // Bump epoch to 2
    j.advance_epoch(stream_key, None).expect("bump epoch");

    // First seq in epoch 2 must continue from the stream-wide high-water (3),
    // NOT reset to 1.
    let s = j.next_seq(stream_key).expect("seq after epoch bump");
    assert_eq!(
        s, 3,
        "seq is stream-wide and must not reset after epoch bump"
    );
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
        b"aa01line",
        "RAW",
    )
    .expect("insert event epoch 1");

    // Bump to epoch 2
    j.advance_epoch(stream_key, None).expect("bump epoch");

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
        for _ in 0..3 {
            let seq = j.next_seq("192.168.2.50").unwrap();
            j.insert_event("192.168.2.50", 1, seq, None, b"line", "RAW")
                .unwrap();
        }
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

    // Write in epoch 1 (seq 1), bump to 2, write in epoch 2 (seq 2)
    {
        let mut j = open_journal_at(&path);
        j.ensure_stream_state("10.0.0.1", 1).unwrap();
        let s1 = j.next_seq("10.0.0.1").unwrap();
        j.insert_event("10.0.0.1", 1, s1, None, b"line", "RAW")
            .unwrap();
        j.advance_epoch("10.0.0.1", None).unwrap();
        let s2 = j.next_seq("10.0.0.1").unwrap(); // seq=2 (stream-wide) in epoch 2
        j.insert_event("10.0.0.1", 2, s2, None, b"line", "RAW")
            .unwrap();
    }

    // Reopen — should be in epoch 2, next stream-wide seq is 3
    {
        let mut j = open_journal_at(&path);
        let (epoch, next_seq) = j.current_epoch_and_next_seq("10.0.0.1").expect("state");
        assert_eq!(epoch, 2, "epoch must be persisted");
        assert_eq!(next_seq, 3, "next_seq after reopen continues stream-wide");
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
        b"aa400000000123450a2a01123018455927a7",
        "RAW",
    )
    .expect("insert");

    let events = j
        .read_events_after("192.168.2.10", 0, usize::MAX)
        .expect("unacked");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].stream_key, "192.168.2.10");
    assert_eq!(events[0].stream_epoch, 1);
    assert_eq!(events[0].seq, seq);
    assert_eq!(
        events[0].reader_timestamp,
        Some("2026-01-01T12:00:00Z".to_owned())
    );
    assert_eq!(
        events[0].raw_frame,
        b"aa400000000123450a2a01123018455927a7".to_vec()
    );
    assert_eq!(events[0].read_type, "RAW");
}

#[test]
fn empty_raw_frame_is_rejected() {
    let (mut j, _f) = open_journal();
    j.ensure_stream_state("192.168.2.11", 1).unwrap();
    let seq = j.next_seq("192.168.2.11").unwrap();

    let result = j.insert_event("192.168.2.11", 1, seq, None, b"", "RAW");
    assert!(result.is_err(), "empty raw_frame must be rejected");
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
        j.insert_event("192.168.2.20", 1, seq, None, b"line", "RAW")
            .unwrap();
        assert_eq!(seq, i);
    }

    // Ack through seq 3
    j.update_receiver_stream_cursor("test-receiver", "192.168.2.20", 3)
        .expect("ack");

    // Replay starts from after the ack cursor (seq 3), so seq 4 and 5 are unacked
    let acked_seq = j.min_acked_through_seq("192.168.2.20").expect("ack cursor");
    assert_eq!(acked_seq, 3);

    let unacked = j
        .read_events_after("192.168.2.20", acked_seq, usize::MAX)
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
