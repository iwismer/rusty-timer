//! Tests for the transactional `Journal::append_read` API.
//!
//! `append_read` allocates the next stream-wide sequence number and inserts
//! the event in a single `BEGIN IMMEDIATE` transaction. Sequence numbers are
//! monotonic within a stream and never reset across epoch bumps (epoch is
//! metadata). If the transaction aborts, no durable gap is left behind.

use forwarder::storage::journal::Journal;
use std::sync::Arc;
use std::sync::Barrier;
use std::thread;

/// Helper: open a Journal backed by a temporary directory on disk.
fn open_temp_journal() -> (Journal, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("journal.db");
    let journal = Journal::open(&path).expect("open journal");
    (journal, dir)
}

#[test]
fn seq_is_monotonic_within_stream() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "10.0.0.10:10000";
    journal
        .ensure_stream_state(stream_key, 1)
        .expect("ensure stream state");

    let mut seqs = Vec::new();
    for i in 0..5 {
        let frame = format!("frame-{i}");
        let (_epoch, seq) = journal
            .append_read(stream_key, None, frame.as_bytes(), "RAW")
            .expect("append_read");
        seqs.push(seq);
    }

    assert_eq!(seqs, vec![1, 2, 3, 4, 5], "seq must be monotonic from 1");
}

#[test]
fn seq_continues_across_epoch_bump() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "10.0.0.11:10000";
    journal
        .ensure_stream_state(stream_key, 1)
        .expect("ensure stream state");

    let (e1, s1) = journal
        .append_read(stream_key, None, b"a", "RAW")
        .expect("append 1");
    let (e2, s2) = journal
        .append_read(stream_key, None, b"b", "RAW")
        .expect("append 2");
    assert_eq!((e1, s1), (1, 1));
    assert_eq!((e2, s2), (1, 2));

    journal.bump_epoch(stream_key, 2).expect("bump epoch");

    let (e3, s3) = journal
        .append_read(stream_key, None, b"c", "RAW")
        .expect("append 3");
    let (e4, s4) = journal
        .append_read(stream_key, None, b"d", "RAW")
        .expect("append 4");

    assert_eq!(e3, 2, "epoch must advance to 2");
    assert_eq!(e4, 2, "epoch must advance to 2");
    assert_eq!(s3, 3, "seq must continue across epoch bump, not reset");
    assert_eq!(s4, 4, "seq must continue across epoch bump, not reset");
}

#[test]
fn no_gap_if_txn_aborts() {
    let (mut journal, _dir) = open_temp_journal();
    let stream_key = "10.0.0.12:10000";
    journal
        .ensure_stream_state(stream_key, 1)
        .expect("ensure stream state");

    let (_e1, s1) = journal
        .append_read(stream_key, None, b"a", "RAW")
        .expect("append 1");
    let (_e2, s2) = journal
        .append_read(stream_key, None, b"b", "RAW")
        .expect("append 2");
    assert_eq!((s1, s2), (1, 2));

    // Inject a failure: an empty raw frame is rejected *inside* the
    // transaction after the next seq (3) has been allocated, forcing a
    // rollback so seq 3 is never committed.
    let aborted = journal.append_read(stream_key, None, b"", "RAW");
    assert!(aborted.is_err(), "empty frame must abort the transaction");

    // The next successful append must reuse the uncommitted seq: no durable
    // gap is left behind by the aborted transaction.
    let (_e3, s3) = journal
        .append_read(stream_key, None, b"c", "RAW")
        .expect("append 3");
    assert_eq!(s3, 3, "aborted txn must not leave a durable seq gap");

    assert_eq!(
        journal.event_count(stream_key).unwrap(),
        3,
        "only the three committed events should exist"
    );
}

#[test]
fn two_writers_no_duplicate_seq() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let path = dir.path().join("journal.db");
    let stream_key = "10.0.0.13:10000";

    // Initialize stream state once before concurrent writers start.
    {
        let mut journal = Journal::open(&path).expect("open journal");
        journal
            .ensure_stream_state(stream_key, 1)
            .expect("ensure stream state");
    }

    const WRITERS: usize = 4;
    const PER_WRITER: usize = 25;

    let barrier = Arc::new(Barrier::new(WRITERS));
    let mut handles = Vec::new();
    for w in 0..WRITERS {
        let path = path.clone();
        let barrier = Arc::clone(&barrier);
        let stream_key = stream_key.to_owned();
        handles.push(thread::spawn(move || {
            let mut journal = Journal::open(&path).expect("open journal");
            barrier.wait();
            let mut seqs = Vec::with_capacity(PER_WRITER);
            for i in 0..PER_WRITER {
                let frame = format!("w{w}-{i}");
                let (_epoch, seq) = journal
                    .append_read(&stream_key, None, frame.as_bytes(), "RAW")
                    .expect("append_read");
                seqs.push(seq);
            }
            seqs
        }));
    }

    let mut all_seqs: Vec<i64> = Vec::new();
    for h in handles {
        all_seqs.extend(h.join().expect("writer thread"));
    }

    all_seqs.sort_unstable();
    let expected: Vec<i64> = (1..=(WRITERS * PER_WRITER) as i64).collect();
    assert_eq!(
        all_seqs, expected,
        "every seq must be unique and contiguous with no duplicates"
    );
}
