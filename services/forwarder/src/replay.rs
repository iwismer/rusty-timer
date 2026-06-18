//! Replay engine: computes the set of pending (unacked) events to send.
//!
//! Used by the P2P session to determine which events need to be
//! (re-)transmitted after a reconnect or on initial connect.

use crate::storage::journal::{Journal, JournalError, JournalEvent};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A group of pending events for a single (stream_key, stream_epoch) pair.
#[derive(Debug)]
pub struct ReplayResult {
    pub stream_key: String,
    pub stream_epoch: i64,
    pub events: Vec<JournalEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapNotice {
    pub requested_cursor: i64,
    pub earliest: i64,
    pub latest: i64,
}

#[derive(Debug)]
pub struct CursorReplayBatch {
    pub records: Vec<JournalEvent>,
    pub earliest: i64,
    pub latest: i64,
    pub gap: Option<GapNotice>,
}

// ---------------------------------------------------------------------------
// ReplayEngine
// ---------------------------------------------------------------------------

/// Computes pending events from the journal.
///
/// "Pending" = events that exist in the journal but have NOT been acked by
/// a receiver cursor (i.e., their seq is greater than the receiver's durable cursor).
pub struct ReplayEngine;

impl ReplayEngine {
    pub fn new() -> Self {
        ReplayEngine
    }

    /// Return durable records strictly after `cursor`, capped by `max`.
    ///
    /// The journal is the source for both replay and live catch-up: callers keep
    /// advancing the cursor and call this again after append wake-ups. If the
    /// cursor is older than the retained prefix, the batch carries a gap notice
    /// and no records so the caller can jump to `earliest - 1` explicitly.
    pub fn read_after(
        &self,
        journal: &Journal,
        stream_id: &str,
        cursor: i64,
        max: usize,
    ) -> Result<CursorReplayBatch, JournalError> {
        let earliest = journal.retention_state(stream_id)?.earliest_available_seq;
        let latest = journal.latest_committed_seq(stream_id)?;

        if cursor < earliest - 1 {
            return Ok(CursorReplayBatch {
                records: Vec::new(),
                earliest,
                latest,
                gap: Some(GapNotice {
                    requested_cursor: cursor,
                    earliest,
                    latest,
                }),
            });
        }

        Ok(CursorReplayBatch {
            records: journal.read_events_after(stream_id, cursor, max)?,
            earliest,
            latest,
            gap: None,
        })
    }

    /// Return all pending events for a stream, grouped by epoch.
    ///
    /// Each `ReplayResult` covers one epoch. If there are unacked events in
    /// multiple epochs (e.g., old epoch + new epoch after bump), all are returned.
    ///
    /// Epochs are returned in ascending order (oldest first = acked drains first).
    pub fn pending_events(
        &self,
        journal: &Journal,
        stream_key: &str,
    ) -> Result<Vec<ReplayResult>, JournalError> {
        let (acked_epoch, acked_seq) = journal.ack_cursor(stream_key)?;
        let mut results: Vec<ReplayResult> = Vec::new();

        // 1. Replay old-epoch backlog (events in acked_epoch with seq > acked_seq)
        if acked_epoch > 0 {
            let events = journal.unacked_events(stream_key, acked_epoch, acked_seq)?;
            if !events.is_empty() {
                results.push(ReplayResult {
                    stream_key: stream_key.to_owned(),
                    stream_epoch: acked_epoch,
                    events,
                });
            }
        }

        // 2. Replay events in epochs after acked_epoch (new epoch events, all from seq > 0)
        // We query a range of possible epochs. Since we don't have a direct "list epochs"
        // method, use a helper: get all journal entries for stream_key with epoch > acked_epoch.
        let newer_events = journal.unacked_events_across_epochs(stream_key, acked_epoch)?;

        // Group by epoch
        let mut epoch_groups: std::collections::BTreeMap<i64, Vec<JournalEvent>> =
            std::collections::BTreeMap::new();
        for ev in newer_events {
            epoch_groups.entry(ev.stream_epoch).or_default().push(ev);
        }

        for (epoch, events) in epoch_groups {
            if !events.is_empty() {
                results.push(ReplayResult {
                    stream_key: stream_key.to_owned(),
                    stream_epoch: epoch,
                    events,
                });
            }
        }

        Ok(results)
    }
}

impl Default for ReplayEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::ReplayEngine;
    use crate::storage::journal::{Journal, RegistryStreamRestore, StreamStartupStatus};
    use tempfile::NamedTempFile;

    fn make_journal() -> (Journal, NamedTempFile) {
        let file = NamedTempFile::new().expect("temp file");
        let journal = Journal::open(file.path()).expect("open journal");
        (journal, file)
    }

    #[test]
    fn replay_returns_after_cursor() {
        let (mut journal, _file) = make_journal();
        journal.ensure_stream_state("stream-a", 1).unwrap();

        for frame in [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()] {
            journal.append_read("stream-a", None, frame, "RAW").unwrap();
        }

        let batch = ReplayEngine::new()
            .read_after(&journal, "stream-a", 1, 10)
            .unwrap();

        assert_eq!(batch.earliest, 1);
        assert_eq!(batch.latest, 3);
        assert!(batch.gap.is_none());
        assert_eq!(batch.records.len(), 2);
        assert_eq!(batch.records[0].seq, 2);
        assert_eq!(batch.records[1].seq, 3);
    }

    #[test]
    fn cursor_below_earliest_yields_gap() {
        let (mut journal, _file) = make_journal();
        journal.ensure_stream_state("stream-gap", 1).unwrap();

        for frame in [b"one".as_slice(), b"two".as_slice(), b"three".as_slice()] {
            journal
                .append_read("stream-gap", None, frame, "RAW")
                .unwrap();
        }
        journal.update_ack_cursor("stream-gap", 1, 2).unwrap();
        journal.prune_acked("stream-gap", 10).unwrap();

        let batch = ReplayEngine::new()
            .read_after(&journal, "stream-gap", 0, 10)
            .unwrap();

        assert_eq!(batch.earliest, 3);
        assert_eq!(batch.latest, 3);
        assert!(batch.records.is_empty());
        let gap = batch.gap.expect("cursor below retention floor should gap");
        assert_eq!(gap.requested_cursor, 0);
        assert_eq!(gap.earliest, 3);
        assert_eq!(gap.latest, 3);
    }

    #[test]
    fn journal_loss_forces_new_stream_id() {
        let first_dir = tempfile::tempdir().unwrap();
        let first_path = first_dir.path().join("journal.db");
        let mut journal = Journal::open(&first_path).unwrap();
        let startup = journal
            .ensure_stream_after_startup("reader-a", None, "stream-old", 1, None)
            .unwrap();
        assert_eq!(startup.stream_id, "stream-old");
        journal
            .append_read(&startup.stream_id, None, b"old-one", "RAW")
            .unwrap();
        journal
            .append_read(&startup.stream_id, None, b"old-two", "RAW")
            .unwrap();

        let lost_dir = tempfile::tempdir().unwrap();
        let lost_path = lost_dir.path().join("journal.db");
        let mut lost_journal = Journal::open(&lost_path).unwrap();
        assert!(
            lost_journal
                .ensure_stream_after_startup("reader-a", Some("stream-old"), "stream-old", 1, None)
                .is_err(),
            "missing local state must not restart the prior stream at seq 1"
        );

        let recovered = lost_journal
            .ensure_stream_after_startup("reader-a", Some("stream-old"), "stream-new", 1, None)
            .unwrap();

        assert_eq!(recovered.stream_id, "stream-new");
        assert_eq!(recovered.status, StreamStartupStatus::ReplacedLostJournal);
        let (_epoch, seq) = lost_journal
            .append_read(&recovered.stream_id, None, b"new-one", "RAW")
            .unwrap();
        assert_eq!(seq, 1);
        assert_eq!(lost_journal.event_count("stream-old").unwrap(), 0);
    }

    #[test]
    fn registry_restore_seeds_high_water_on_fresh_journal() {
        let (mut journal, _file) = make_journal();

        let startup = journal
            .ensure_stream_after_startup(
                "reader-a",
                Some("stream-old"),
                "stream-old",
                1,
                Some(RegistryStreamRestore {
                    stream_id: "stream-old",
                    epoch: 7,
                    next_seq: 42,
                }),
            )
            .unwrap();

        assert_eq!(startup.stream_id, "stream-old");
        assert_eq!(startup.status, StreamStartupStatus::RestoredFromRegistry);
        let (epoch, seq) = journal
            .append_read(&startup.stream_id, None, b"restored-one", "RAW")
            .unwrap();
        assert_eq!(epoch, 7);
        assert_eq!(seq, 42);
    }

    #[test]
    fn latest_tracks_appends() {
        let (mut journal, _file) = make_journal();
        journal.ensure_stream_state("stream-latest", 1).unwrap();
        let engine = ReplayEngine::new();

        let empty = engine.read_after(&journal, "stream-latest", 0, 10).unwrap();
        assert_eq!(empty.latest, 0);

        journal
            .append_read("stream-latest", None, b"first", "RAW")
            .unwrap();
        let after_first = engine.read_after(&journal, "stream-latest", 0, 10).unwrap();
        assert_eq!(after_first.latest, 1);

        journal
            .append_read("stream-latest", None, b"second", "RAW")
            .unwrap();
        let after_second = engine.read_after(&journal, "stream-latest", 1, 10).unwrap();
        assert_eq!(after_second.latest, 2);
        assert_eq!(after_second.records.len(), 1);
        assert_eq!(after_second.records[0].seq, 2);
    }

    #[test]
    fn pending_events_groups_and_orders_epochs_when_ack_cursor_is_zero() {
        let (mut journal, _file) = make_journal();
        journal.ensure_stream_state("10.0.0.10:10000", 1).unwrap();

        let seq1 = journal.next_seq("10.0.0.10:10000").unwrap();
        journal
            .insert_event("10.0.0.10:10000", 1, seq1, None, b"epoch1-seq1", "RAW")
            .unwrap();
        let seq2 = journal.next_seq("10.0.0.10:10000").unwrap();
        journal
            .insert_event("10.0.0.10:10000", 1, seq2, None, b"epoch1-seq2", "RAW")
            .unwrap();

        journal.bump_epoch("10.0.0.10:10000", 2).unwrap();
        let seq3 = journal.next_seq("10.0.0.10:10000").unwrap();
        journal
            .insert_event("10.0.0.10:10000", 2, seq3, None, b"epoch2-seq1", "RAW")
            .unwrap();

        let replay = ReplayEngine::new()
            .pending_events(&journal, "10.0.0.10:10000")
            .unwrap();

        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0].stream_epoch, 1);
        assert_eq!(replay[1].stream_epoch, 2);
        assert_eq!(replay[0].events.len(), 2);
        assert_eq!(replay[1].events.len(), 1);
        assert_eq!(replay[0].events[0].seq, 1);
        assert_eq!(replay[0].events[1].seq, 2);
        assert_eq!(replay[1].events[0].seq, 3);
    }

    #[test]
    fn pending_events_includes_old_epoch_backlog_and_newer_epochs() {
        let (mut journal, _file) = make_journal();
        journal.ensure_stream_state("10.0.0.20:10000", 1).unwrap();

        for _ in 0..3 {
            let seq = journal.next_seq("10.0.0.20:10000").unwrap();
            journal
                .insert_event("10.0.0.20:10000", 1, seq, None, b"epoch1", "RAW")
                .unwrap();
        }
        journal.update_ack_cursor("10.0.0.20:10000", 1, 1).unwrap();

        journal.bump_epoch("10.0.0.20:10000", 2).unwrap();
        for _ in 0..2 {
            let seq = journal.next_seq("10.0.0.20:10000").unwrap();
            journal
                .insert_event("10.0.0.20:10000", 2, seq, None, b"epoch2", "RAW")
                .unwrap();
        }

        let replay = ReplayEngine::new()
            .pending_events(&journal, "10.0.0.20:10000")
            .unwrap();

        assert_eq!(replay.len(), 2);
        assert_eq!(replay[0].stream_epoch, 1);
        assert_eq!(replay[0].events.len(), 2);
        assert_eq!(replay[0].events[0].seq, 2);
        assert_eq!(replay[0].events[1].seq, 3);
        assert_eq!(replay[1].stream_epoch, 2);
        assert_eq!(replay[1].events.len(), 2);
        assert_eq!(replay[1].events[0].seq, 4);
        assert_eq!(replay[1].events[1].seq, 5);
    }

    #[test]
    fn pending_events_is_empty_for_initialized_stream_without_events() {
        let (mut journal, _file) = make_journal();
        journal.ensure_stream_state("10.0.0.30:10000", 1).unwrap();

        let replay = ReplayEngine::new()
            .pending_events(&journal, "10.0.0.30:10000")
            .unwrap();

        assert!(replay.is_empty());
    }
}
