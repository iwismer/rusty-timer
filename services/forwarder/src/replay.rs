//! Replay engine: computes the set of pending (unacked) events to send.
//!
//! Used by the uplink session to determine which events need to be
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

// ---------------------------------------------------------------------------
// ReplayEngine
// ---------------------------------------------------------------------------

/// Computes pending events from the journal.
///
/// "Pending" = events that exist in the journal but have NOT been acked by
/// the server (i.e., their seq is > the acked_through_seq for that epoch).
pub struct ReplayEngine;

impl ReplayEngine {
    pub fn new() -> Self {
        ReplayEngine
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
    use crate::storage::journal::Journal;
    use tempfile::NamedTempFile;

    fn make_journal() -> (Journal, NamedTempFile) {
        let file = NamedTempFile::new().expect("temp file");
        let journal = Journal::open(file.path()).expect("open journal");
        (journal, file)
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
        assert_eq!(replay[1].events[0].seq, 1);
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
        assert_eq!(replay[1].events[0].seq, 1);
        assert_eq!(replay[1].events[1].seq, 2);
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
