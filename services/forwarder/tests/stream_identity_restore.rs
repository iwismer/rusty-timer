//! Startup stream-identity restore after journal loss.
//!
//! If the journal file is lost/recreated, the same stream key (reader network
//! address) must NOT restart at seq 1: receivers dedup on `(stream_id, seq)`
//! and would silently discard the new reads as duplicates. Startup restores
//! epoch/next_seq from the server registry high-water (plus fixed slack), and
//! falls back to seq 1 — loudly when the registry was unavailable.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use forwarder::storage::journal::Journal;
use forwarder::storage::restore::{
    RESTORE_SEQ_SLACK, RegistryFetch, RegistryStreamRecord, StreamRestoreOutcome,
    fetch_registry_snapshot_with_retries, restore_streams_at_startup,
};
use forwarder::ui_events::ForwarderUiEvent;

const STREAM_KEY: &str = "10.0.0.5:10000";

fn test_logger() -> Arc<rt_ui_log::UiLogger<ForwarderUiEvent>> {
    let (tx, _) = tokio::sync::broadcast::channel(64);
    Arc::new(rt_ui_log::UiLogger::with_buffer(
        tx,
        |entry| ForwarderUiEvent::LogEntry { entry },
        100,
    ))
}

/// Simulate journal loss: journal with appended events is deleted and a fresh
/// one is created at the same path (same stream key).
fn lose_journal(dir: &tempfile::TempDir) -> (Journal, i64) {
    let path = dir.path().join("journal.sqlite");
    let mut journal = Journal::open(&path).unwrap();
    journal.ensure_stream_state(STREAM_KEY, 1).unwrap();
    let mut last_seq = 0;
    for _ in 0..5 {
        let (_, seq) = journal
            .append_read(STREAM_KEY, None, b"aa0000000000000000000000000000", "RAW")
            .unwrap();
        last_seq = seq;
    }
    assert_eq!(last_seq, 5, "expected seq to have advanced");
    drop(journal);

    // Delete the journal (plus SQLite sidecar files) — the loss scenario.
    for suffix in ["", "-wal", "-shm"] {
        let mut os = path.clone().into_os_string();
        os.push(suffix);
        let _ = std::fs::remove_file(std::path::PathBuf::from(os));
    }

    let journal = Journal::open(&path).unwrap();
    (journal, last_seq + 1) // pre-loss next_seq
}

/// Case (a): the server has a record for the stream — restore continues from
/// the registry high-water plus slack, never back at seq 1.
#[test]
fn restore_continues_seq_from_registry_high_water_after_journal_loss() {
    let dir = tempfile::tempdir().unwrap();
    let (mut journal, pre_loss_next_seq) = lose_journal(&dir);

    let fetch = RegistryFetch::Snapshot(vec![RegistryStreamRecord {
        stream_id: STREAM_KEY.to_owned(),
        epoch: 3,
        next_seq: u64::try_from(pre_loss_next_seq).unwrap(),
    }]);
    let logger = test_logger();
    let outcomes = restore_streams_at_startup(
        &mut journal,
        &[STREAM_KEY.to_owned()],
        &fetch,
        Some(&logger),
    )
    .unwrap();

    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].0, STREAM_KEY);
    assert!(
        matches!(outcomes[0].1, StreamRestoreOutcome::Restored { .. }),
        "expected registry restore, got {:?}",
        outcomes[0].1
    );

    let (epoch, seq) = journal
        .append_read(STREAM_KEY, None, b"aa0000000000000000000000000000", "RAW")
        .unwrap();
    assert_eq!(epoch, 3, "restored epoch must be preserved");
    assert_eq!(
        seq,
        pre_loss_next_seq + RESTORE_SEQ_SLACK,
        "append must continue from restored next_seq + slack, not 1"
    );
}

/// Case (b): server reachable but has no record for this stream — expected
/// first boot; seed seq 1 without any error-level UI log.
#[test]
fn restore_seeds_seq_one_when_server_has_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let mut journal = Journal::open(&path).unwrap();

    let fetch = RegistryFetch::Snapshot(vec![]);
    let logger = test_logger();
    let outcomes = restore_streams_at_startup(
        &mut journal,
        &[STREAM_KEY.to_owned()],
        &fetch,
        Some(&logger),
    )
    .unwrap();

    assert_eq!(outcomes[0].1, StreamRestoreOutcome::SeededFirstBoot);
    let (_, seq) = journal
        .append_read(STREAM_KEY, None, b"aa0000000000000000000000000000", "RAW")
        .unwrap();
    assert_eq!(seq, 1);
    assert!(
        !logger.entries().iter().any(|e| e.contains("[ERROR]")),
        "first boot must not emit an error UI log: {:?}",
        logger.entries()
    );
}

/// First boot with a server configured but no P2P identity yet: the registry
/// cannot hold records for a freshly generated identity, so seeding at seq 1
/// is benign — info log only, no warn/error noise.
#[test]
fn restore_fresh_identity_seeds_seq_one_without_error_noise() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let mut journal = Journal::open(&path).unwrap();

    let logger = test_logger();
    let outcomes = restore_streams_at_startup(
        &mut journal,
        &[STREAM_KEY.to_owned()],
        &RegistryFetch::FreshIdentity,
        Some(&logger),
    )
    .unwrap();

    assert_eq!(outcomes[0].1, StreamRestoreOutcome::SeededFirstBoot);
    let (_, seq) = journal
        .append_read(STREAM_KEY, None, b"aa0000000000000000000000000000", "RAW")
        .unwrap();
    assert_eq!(seq, 1);
    assert!(
        !logger
            .entries()
            .iter()
            .any(|e| e.contains("[ERROR]") || e.contains("[WARN]")),
        "fresh-identity first boot must not emit warn/error UI logs: {:?}",
        logger.entries()
    );
}

/// Case (c): registry unavailable (unreachable, errored, or 404 from an older
/// server) — seed seq 1 but emit a loud error UI log warning that receiver
/// dedup may discard reads.
#[test]
fn restore_seeds_seq_one_and_logs_error_when_registry_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let (mut journal, _) = lose_journal(&dir);

    let logger = test_logger();
    let outcomes = restore_streams_at_startup(
        &mut journal,
        &[STREAM_KEY.to_owned()],
        &RegistryFetch::Unavailable,
        Some(&logger),
    )
    .unwrap();

    assert_eq!(outcomes[0].1, StreamRestoreOutcome::SeededWithoutRegistry);
    let (_, seq) = journal
        .append_read(STREAM_KEY, None, b"aa0000000000000000000000000000", "RAW")
        .unwrap();
    assert_eq!(seq, 1);

    let entries = logger.entries();
    let error_entry = entries
        .iter()
        .find(|e| e.contains("[ERROR]") && e.contains(STREAM_KEY));
    assert!(
        error_entry.is_some_and(|e| e.contains("discard")),
        "expected loud error UI log about receiver dedup discarding reads, got {entries:?}"
    );
}

/// Streams that still have local journal state are untouched, even when the
/// registry snapshot carries a (stale, lower) high-water for them.
#[test]
fn restore_leaves_existing_streams_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.sqlite");
    let mut journal = Journal::open(&path).unwrap();
    journal.ensure_stream_state(STREAM_KEY, 1).unwrap();
    for _ in 0..3 {
        journal
            .append_read(STREAM_KEY, None, b"aa0000000000000000000000000000", "RAW")
            .unwrap();
    }

    let fetch = RegistryFetch::Snapshot(vec![RegistryStreamRecord {
        stream_id: STREAM_KEY.to_owned(),
        epoch: 9,
        next_seq: 2,
    }]);
    let outcomes =
        restore_streams_at_startup(&mut journal, &[STREAM_KEY.to_owned()], &fetch, None).unwrap();

    assert_eq!(outcomes[0].1, StreamRestoreOutcome::Existing);
    let (epoch, seq) = journal
        .append_read(STREAM_KEY, None, b"aa0000000000000000000000000000", "RAW")
        .unwrap();
    assert_eq!(epoch, 1);
    assert_eq!(seq, 4, "existing local state must win over the snapshot");
}

/// The fetch wrapper retries a bounded number of times and then reports
/// `Unavailable`; it never retries after a success.
#[tokio::test]
async fn fetch_retries_are_bounded_then_unavailable() {
    let calls = Arc::new(AtomicU32::new(0));
    let calls_in_fetch = Arc::clone(&calls);
    let fetch = fetch_registry_snapshot_with_retries(
        move || {
            let calls = Arc::clone(&calls_in_fetch);
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<Vec<RegistryStreamRecord>, String>("connection refused".to_owned())
            }
        },
        3,
        Duration::ZERO,
    )
    .await;

    assert_eq!(fetch, RegistryFetch::Unavailable);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        3,
        "exactly 3 bounded attempts"
    );
}

/// A success on a later attempt yields the snapshot (no further retries).
#[tokio::test]
async fn fetch_succeeds_after_transient_failures() {
    let calls = Arc::new(AtomicU32::new(0));
    let calls_in_fetch = Arc::clone(&calls);
    let fetch = fetch_registry_snapshot_with_retries(
        move || {
            let calls = Arc::clone(&calls_in_fetch);
            async move {
                if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err::<Vec<RegistryStreamRecord>, String>("boom".to_owned())
                } else {
                    Ok(vec![RegistryStreamRecord {
                        stream_id: STREAM_KEY.to_owned(),
                        epoch: 1,
                        next_seq: 42,
                    }])
                }
            }
        },
        3,
        Duration::ZERO,
    )
    .await;

    assert_eq!(
        fetch,
        RegistryFetch::Snapshot(vec![RegistryStreamRecord {
            stream_id: STREAM_KEY.to_owned(),
            epoch: 1,
            next_seq: 42,
        }])
    );
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}
