//! Dedicated SQLite writer: a single OS thread owning one write connection,
//! batching all streams' persist commands into group commits (one fsync per
//! commit window) while preserving the insert-before-ack durability contract.
//!
//! Sessions send [`WriteCommand`]s over a bounded mpsc channel and await the
//! oneshot reply. Replies are sent **only after a successful `COMMIT`** — a
//! reply carrying `Ok` is the caller's proof the rows are durable, so it may
//! ack the forwarder (which then prunes). Never reply `Ok` for a rolled-back
//! row.
//!
//! Cursor tracking is in-memory ([`CursorState`]), replacing the per-batch
//! `advance_cursor_contiguous_prefix` table scan. Cursor mutations are staged
//! per transaction and applied to the live map only after `COMMIT` succeeds.

use std::collections::BTreeSet;

use crate::p2p_session::EventFact;

/// In-memory contiguous-cursor tracker; replaces the
/// `advance_cursor_contiguous_prefix` scan on the hot path.
///
/// `last_contiguous` is the durable ack cursor: every seq `<=` it is stored.
/// `pending` holds stored seqs above the contiguous prefix (arrival gaps).
#[derive(Clone, Debug, Default)]
pub struct CursorState {
    last_contiguous: i64,
    pending: BTreeSet<i64>,
}

impl CursorState {
    /// A cursor with no stored rows above `last_contiguous`.
    pub fn new(last_contiguous: i64) -> Self {
        Self {
            last_contiguous,
            pending: BTreeSet::new(),
        }
    }

    /// Rebuild from the durable store: the persisted cursor row plus every
    /// stored seq above it (`SELECT seq ... WHERE seq > cursor`).
    pub fn rebuild(last_contiguous: i64, stored_above: impl IntoIterator<Item = i64>) -> Self {
        let mut state = Self::new(last_contiguous);
        for seq in stored_above {
            state.observe(seq);
        }
        state
    }

    /// Record one stored seq. Duplicates and seqs at or below the contiguous
    /// prefix are no-ops (redelivered rows hit this constantly under
    /// at-least-once), so `pending` never grows from retransmits.
    pub fn observe(&mut self, seq: i64) {
        if seq <= self.last_contiguous {
            return;
        }
        self.pending.insert(seq);
        while self
            .pending
            .first()
            .is_some_and(|&next| next == self.last_contiguous + 1)
        {
            self.last_contiguous += 1;
            self.pending.pop_first();
        }
    }

    /// The durable contiguous cursor (ack watermark).
    pub fn durable_cursor(&self) -> i64 {
        self.last_contiguous
    }

    /// Gap-notice path: jump the cursor forward to `seq` (never backward) and
    /// drop pending seqs the jump absorbed.
    pub fn jump_to(&mut self, seq: i64) {
        if seq <= self.last_contiguous {
            return;
        }
        self.last_contiguous = seq;
        self.pending = self.pending.split_off(&(seq + 1));
        // The jump may have made previously pending seqs contiguous.
        while self
            .pending
            .first()
            .is_some_and(|&next| next == self.last_contiguous + 1)
        {
            self.last_contiguous += 1;
            self.pending.pop_first();
        }
    }
}

/// A record already validated by the session (stream id checked, u64→i64
/// converted, chip id parsed). The writer only persists it; the duplicate
/// *check* (which needs the DB) runs inside the writer's transaction.
#[derive(Clone, Debug)]
pub struct PreparedRecord {
    pub seq: i64,
    pub epoch: i64,
    pub raw_frame: Vec<u8>,
    pub read_kind: String,
    pub reader_timestamp: Option<String>,
    pub received_unix_ms: i64,
    pub chip_id: String,
}

/// A gap notice already validated by the session.
#[derive(Clone, Debug)]
pub struct PreparedGap {
    pub requested_after_seq: i64,
    pub earliest_available_seq: i64,
    pub latest_available_seq: i64,
    pub reason: String,
    pub created_unix_ms: i64,
}

/// Writer-side failure for one command. `ConflictingDuplicate` is per-command
/// (its SAVEPOINT rolled back; other commands in the group commit normally);
/// `Db` failures at commit time fail every command in the group.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("db: {0}")]
    Db(#[from] crate::db::DbError),
    #[error("conflicting duplicate for stream {stream_id} seq {seq}")]
    ConflictingDuplicate { stream_id: String, seq: i64 },
    #[error("writer unavailable: {0}")]
    Closed(String),
}

/// One unit of work for the writer thread.
#[derive(Debug)]
pub enum WriteCommand {
    /// Persist one EventBatch's records and advance the stream cursor. The
    /// reply carries the post-commit durable cursor and inserted facts.
    PersistBatch {
        stream_id: String,
        records: Vec<PreparedRecord>,
        reply: tokio::sync::oneshot::Sender<Result<crate::p2p_session::DurableBatch, WriteError>>,
    },
    /// Persist a gap marker and jump the stream cursor. The reply carries the
    /// post-commit durable cursor.
    PersistGap {
        stream_id: String,
        gap: PreparedGap,
        reply: tokio::sync::oneshot::Sender<Result<i64, WriteError>>,
    },
    /// Trigger a manual PASSIVE WAL checkpoint.
    Checkpoint,
}

/// Build the [`EventFact`]s for the records a batch actually inserted.
pub(crate) fn facts_for(records: &[PreparedRecord], inserted: &[bool]) -> Vec<EventFact> {
    records
        .iter()
        .zip(inserted)
        .filter(|&(_, &ins)| ins)
        .map(|(record, _)| EventFact {
            seq: record.seq,
            epoch: record.epoch,
            received_unix_ms: record.received_unix_ms,
            chip_id: record.chip_id.clone(),
        })
        .collect()
}

#[cfg(test)]
mod cursor_tests {
    use super::*;

    #[test]
    fn contiguous_advance() {
        let mut cursor = CursorState::new(0);
        cursor.observe(1);
        cursor.observe(2);
        cursor.observe(3);
        assert_eq!(cursor.durable_cursor(), 3);
        assert!(cursor.pending.is_empty());
    }

    #[test]
    fn gap_holds_cursor() {
        let mut cursor = CursorState::new(0);
        cursor.observe(1);
        cursor.observe(2);
        cursor.observe(4);
        assert_eq!(cursor.durable_cursor(), 2);
        assert_eq!(cursor.pending, BTreeSet::from([4]));

        cursor.observe(3);
        assert_eq!(cursor.durable_cursor(), 4);
        assert!(cursor.pending.is_empty());
    }

    #[test]
    fn gap_jump_resets() {
        let mut cursor = CursorState::new(0);
        cursor.observe(1);
        cursor.observe(12);
        cursor.observe(14);
        cursor.observe(16);
        cursor.jump_to(14);
        assert_eq!(cursor.durable_cursor(), 14);
        assert_eq!(
            cursor.pending,
            BTreeSet::from([16]),
            "pending seqs at or below the jump target are cleared"
        );

        // A jump backward is ignored.
        cursor.jump_to(3);
        assert_eq!(cursor.durable_cursor(), 14);
    }

    #[test]
    fn jump_absorbs_now_contiguous_pending() {
        let mut cursor = CursorState::new(0);
        cursor.observe(15);
        cursor.observe(16);
        cursor.jump_to(14);
        assert_eq!(
            cursor.durable_cursor(),
            16,
            "a jump to 14 makes stored 15,16 contiguous"
        );
        assert!(cursor.pending.is_empty());
    }

    #[test]
    fn stale_seq_is_noop() {
        let mut cursor = CursorState::new(0);
        cursor.observe(1);
        cursor.observe(2);
        cursor.observe(2);
        cursor.observe(1);
        assert_eq!(cursor.durable_cursor(), 2);
        assert!(
            cursor.pending.is_empty(),
            "redelivered seqs must not grow pending"
        );

        cursor.observe(4);
        cursor.observe(4);
        assert_eq!(cursor.pending, BTreeSet::from([4]));
    }

    #[test]
    fn rebuild_from_db_rows() {
        let cursor = CursorState::rebuild(2, [1, 2, 4]);
        assert_eq!(cursor.durable_cursor(), 2);
        assert_eq!(cursor.pending, BTreeSet::from([4]));

        let contiguous = CursorState::rebuild(2, [3, 4]);
        assert_eq!(
            contiguous.durable_cursor(),
            4,
            "stored rows contiguous with the cursor advance it at rebuild"
        );
    }
}
