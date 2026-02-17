//! Replay engine: computes the set of pending (unacked) events to send.
//!
//! Used by the uplink session to determine which events need to be
//! (re-)transmitted after a reconnect or on initial connect.

use crate::storage::journal::{Journal, JournalEvent, JournalError};

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
        let mut epoch_groups: std::collections::BTreeMap<i64, Vec<JournalEvent>> = std::collections::BTreeMap::new();
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
