//! Durable SQLite journal for forwarder events.
//!
//! The on-disk schema is the clean-slate P2P schema managed by
//! `storage::migrations`. A few methods retain the old `Journal` API so the
//! existing forwarder code keeps compiling until journal allocation is rewritten.

use crate::storage::migrations;
use crate::storage::wake::WakeRegistry;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

const COMPAT_RECEIVER_ID: &str = "__forwarder_server__";

/// Maximum number of candidate rows collected (and deleted) in a single
/// `prune_retention` transaction, per prune category.
///
/// Bounds per-transaction work so a single pruning pass cannot collect an
/// unbounded candidate set or hold a long write transaction on large journals.
/// Remaining rows are handled on subsequent pruning passes.
pub const MAX_PRUNE_BATCH: i64 = 10_000;

#[derive(Debug, Clone, Copy)]
pub struct RetentionPolicy {
    pub min_retention_ms: i64,
    pub max_retention_ms: i64,
    pub emergency_free_disk_bytes: u64,
    pub emergency_max_rows: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct RetentionContext {
    pub now_unix_ms: i64,
    pub free_disk_bytes: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPruneStats {
    pub acked_deleted: i64,
    pub hard_cap_deleted: i64,
    pub emergency_deleted: i64,
    pub forced_gap_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionState {
    pub earliest_available_seq: i64,
    pub forced_gap_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistryStreamRestore<'a> {
    pub stream_id: &'a str,
    pub epoch: i64,
    pub next_seq: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStartupStatus {
    Existing,
    Created,
    RestoredFromRegistry,
    ReplacedLostJournal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamStartup {
    pub stream_id: String,
    pub status: StreamStartupStatus,
}

struct PruneCandidate {
    stream_id: String,
    seq: i64,
    forced: bool,
}

/// A read event retrieved from the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrentEpochMetadata {
    pub epoch: i64,
    pub created_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpochSummary {
    pub epoch: i64,
    pub created_unix_ms: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct JournalEvent {
    pub id: i64,
    pub stream_key: String,
    pub stream_epoch: i64,
    pub seq: i64,
    pub reader_timestamp: Option<String>,
    pub raw_frame: Vec<u8>,
    pub read_type: String,
    pub received_at: String,
}

/// Error type for journal operations.
#[derive(Debug)]
pub enum JournalError {
    Sqlite(rusqlite::Error),
    IntegrityCheckFailed(String),
    InvalidData(String),
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JournalError::Sqlite(e) => write!(f, "SQLite error: {e}"),
            JournalError::IntegrityCheckFailed(s) => write!(f, "Integrity check failed: {s}"),
            JournalError::InvalidData(s) => write!(f, "Invalid data: {s}"),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<rusqlite::Error> for JournalError {
    fn from(e: rusqlite::Error) -> Self {
        JournalError::Sqlite(e)
    }
}

/// Read-only journal API used by replay subscribers.
pub trait ReplayJournal {
    fn retention_state(&self, stream_key: &str) -> Result<RetentionState, JournalError>;

    fn latest_committed_seq(&self, stream_key: &str) -> Result<i64, JournalError>;

    fn read_events_after(
        &self,
        stream_key: &str,
        after_seq: i64,
        max: usize,
    ) -> Result<Vec<JournalEvent>, JournalError>;
}

/// A read-only SQLite journal connection for replay queries.
pub struct ReadJournal {
    conn: Connection,
}

impl ReadJournal {
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        apply_read_pragmas(&conn)?;
        Ok(Self { conn })
    }

    #[cfg(test)]
    pub fn connection_for_test(&self) -> &Connection {
        &self.conn
    }

    pub fn retention_state(&self, stream_key: &str) -> Result<RetentionState, JournalError> {
        retention_state(&self.conn, stream_key)
    }

    pub fn latest_committed_seq(&self, stream_key: &str) -> Result<i64, JournalError> {
        latest_committed_seq(&self.conn, stream_key)
    }

    pub fn read_events_after(
        &self,
        stream_key: &str,
        after_seq: i64,
        max: usize,
    ) -> Result<Vec<JournalEvent>, JournalError> {
        read_events_after(&self.conn, stream_key, after_seq, max)
    }
}

impl ReplayJournal for ReadJournal {
    fn retention_state(&self, stream_key: &str) -> Result<RetentionState, JournalError> {
        self.retention_state(stream_key)
    }

    fn latest_committed_seq(&self, stream_key: &str) -> Result<i64, JournalError> {
        self.latest_committed_seq(stream_key)
    }

    fn read_events_after(
        &self,
        stream_key: &str,
        after_seq: i64,
        max: usize,
    ) -> Result<Vec<JournalEvent>, JournalError> {
        self.read_events_after(stream_key, after_seq, max)
    }
}

/// The durable SQLite journal for a single forwarder instance.
pub struct Journal {
    conn: Connection,
    wake: Arc<WakeRegistry>,
}

impl Journal {
    /// Open (or create) the journal at the given path.
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let conn = Connection::open(path)?;
        migrations::migrate(&conn)?;
        Ok(Journal {
            conn,
            wake: Arc::new(WakeRegistry::new()),
        })
    }

    pub fn open_read_only(path: &Path) -> Result<ReadJournal, JournalError> {
        ReadJournal::open(path)
    }

    #[cfg(test)]
    pub fn set_query_only(&self, on: bool) -> Result<(), JournalError> {
        self.conn.execute_batch(if on {
            "PRAGMA query_only = ON"
        } else {
            "PRAGMA query_only = OFF"
        })?;
        Ok(())
    }

    /// Return a shareable handle to this journal's per-stream wake registry.
    ///
    /// Subscribers clone the `Arc` and call
    /// [`WakeRegistry::subscribe`](crate::storage::wake::WakeRegistry::subscribe)
    /// to receive a `watch` of the latest committed seq for a stream. The watch
    /// value advances only after [`append_read`](Self::append_read) commits.
    #[must_use]
    pub fn wake_registry(&self) -> Arc<WakeRegistry> {
        Arc::clone(&self.wake)
    }

    /// Resolve stream metadata after process startup.
    ///
    /// Returns `Existing` when the prior stream still has local journal state,
    /// `RestoredFromRegistry` when server registry high-water restores the
    /// stream epoch and next seq, `ReplacedLostJournal` when missing local state
    /// requires a new stream id, and `Created` for first startup with no prior
    /// stream. Registry restore takes precedence over the missing-prior-state
    /// new-stream guard because it supplies the durable epoch and next-seq
    /// high-water needed to avoid sequence reuse after journal loss.
    pub fn ensure_stream_after_startup(
        &mut self,
        hardware_reader_id: &str,
        prior_stream_id: Option<&str>,
        new_stream_id: &str,
        initial_epoch: i64,
        registry_restore: Option<RegistryStreamRestore<'_>>,
    ) -> Result<StreamStartup, JournalError> {
        if let Some(stream_id) = prior_stream_id
            && self.stream_exists(stream_id)?
        {
            return Ok(StreamStartup {
                stream_id: stream_id.to_owned(),
                status: StreamStartupStatus::Existing,
            });
        }

        if let Some(restore) = registry_restore {
            validate_stream_seed(restore.stream_id, restore.epoch, restore.next_seq)?;
            let tx = self
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            insert_stream_seed(
                &tx,
                restore.stream_id,
                hardware_reader_id,
                restore.epoch,
                restore.next_seq,
                "registry_restore",
            )?;
            tx.commit()?;
            return Ok(StreamStartup {
                stream_id: restore.stream_id.to_owned(),
                status: StreamStartupStatus::RestoredFromRegistry,
            });
        }

        if prior_stream_id == Some(new_stream_id) {
            return Err(JournalError::InvalidData(
                "journal state for prior stream is missing; a new stream_id is required".to_owned(),
            ));
        }

        validate_stream_seed(new_stream_id, initial_epoch, 1)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        insert_stream_seed(
            &tx,
            new_stream_id,
            hardware_reader_id,
            initial_epoch,
            1,
            "initial",
        )?;
        tx.commit()?;

        Ok(StreamStartup {
            stream_id: new_stream_id.to_owned(),
            status: if prior_stream_id.is_some() {
                StreamStartupStatus::ReplacedLostJournal
            } else {
                StreamStartupStatus::Created
            },
        })
    }

    /// Initialize stream metadata if it does not exist yet.
    pub fn ensure_stream_state(
        &mut self,
        stream_key: &str,
        initial_epoch: i64,
    ) -> Result<(), JournalError> {
        let now_ms = unix_ms();
        self.conn.execute(
            "INSERT OR IGNORE INTO streams
                 (stream_id, hardware_reader_id, network_addr, display_name, reader_connected, created_unix_ms)
             VALUES (?1, ?1, ?1, ?1, 0, ?2)",
            params![stream_key, now_ms],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO stream_epochs
                 (stream_id, epoch, start_seq, end_seq, reason, created_unix_ms)
             VALUES (?1, ?2, 1, NULL, 'initial', ?3)",
            params![stream_key, initial_epoch, now_ms],
        )?;
        self.conn.execute(
            "INSERT OR IGNORE INTO stream_retention
                 (stream_id, earliest_available_seq, forced_gap_count)
             VALUES (?1, 1, 0)",
            params![stream_key],
        )?;
        Ok(())
    }

    /// Return the next stream-wide sequence number.
    ///
    /// The seq is stream-wide and never resets across epochs. It is derived
    /// from durable high-water evidence so that pruning acked events can never
    /// cause a previously-issued seq to be reused: we take the maximum of the
    /// highest live event seq, the highest pruned seq (recorded via
    /// `stream_retention.earliest_available_seq`), and the highest acked seq.
    pub fn next_seq(&mut self, stream_key: &str) -> Result<i64, JournalError> {
        next_seq(&self.conn, stream_key)
    }

    /// Atomically allocate the next stream-wide sequence number and insert a
    /// read event in a single `BEGIN IMMEDIATE` transaction.
    ///
    /// Allocating the seq and inserting the event in one transaction guarantees
    /// there is no window between a separate `next_seq()` and `insert_event()`
    /// where another writer (or a crash) could observe or claim the same seq.
    /// The seq is stream-wide and never resets across epoch bumps; the epoch is
    /// recorded on the event as metadata. Returns the `(epoch, seq)` assigned to
    /// the event.
    ///
    /// If anything fails after the seq is computed (for example, an invalid
    /// frame or a constraint violation), the transaction rolls back and nothing
    /// is committed, so the allocated seq is reused by the next append and no
    /// durable gap is left behind.
    pub fn append_read(
        &mut self,
        stream_key: &str,
        reader_timestamp: Option<&str>,
        raw_frame: &[u8],
        read_type: &str,
    ) -> Result<(i64, i64), JournalError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let epoch = current_epoch(&tx, stream_key)?;
        let seq = next_seq(&tx, stream_key)?;

        // Validate inside the transaction (after seq allocation) so a rejected
        // frame rolls back cleanly without committing the allocated seq.
        if raw_frame.is_empty() {
            return Err(JournalError::InvalidData(
                "raw_frame must not be empty".to_owned(),
            ));
        }

        tx.execute(
            "INSERT INTO events
                 (stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                stream_key,
                seq,
                epoch,
                raw_frame,
                read_type,
                reader_timestamp,
                unix_ms(),
            ],
        )?;
        tx.commit()?;
        // Publish the wake-up only after the transaction has committed so a
        // subscriber can never observe a seq that later rolled back.
        self.wake.notify_committed(stream_key, seq as u64);
        Ok((epoch, seq))
    }

    /// Bump the stream epoch without deleting prior events.
    ///
    /// Closes the current open epoch and opens `new_epoch`. The new epoch must
    /// be strictly greater than the current epoch; otherwise an error is
    /// returned rather than silently ignoring the conflict. Both the close and
    /// open run in a single transaction.
    pub fn bump_epoch(&mut self, stream_key: &str, new_epoch: i64) -> Result<(), JournalError> {
        let current = self.current_epoch(stream_key)?;
        if new_epoch <= current {
            return Err(JournalError::InvalidData(format!(
                "new epoch {new_epoch} must be greater than current epoch {current}"
            )));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let start_seq = next_seq(&tx, stream_key)?;
        tx.execute(
            "UPDATE stream_epochs
             SET end_seq = COALESCE(end_seq, ?2 - 1)
             WHERE stream_id = ?1 AND end_seq IS NULL",
            params![stream_key, start_seq],
        )?;
        tx.execute(
            "INSERT INTO stream_epochs
                 (stream_id, epoch, start_seq, end_seq, reason, created_unix_ms)
             VALUES (?1, ?2, ?3, NULL, 'reset', ?4)",
            params![stream_key, new_epoch, start_seq, unix_ms()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Return the current epoch and next sequence number for a stream.
    pub fn current_epoch_and_next_seq(
        &mut self,
        stream_key: &str,
    ) -> Result<(i64, i64), JournalError> {
        let epoch = self.current_epoch(stream_key)?;
        let next_seq = self.next_seq(stream_key)?;
        Ok((epoch, next_seq))
    }

    /// Insert a read event.
    pub fn insert_event(
        &mut self,
        stream_key: &str,
        stream_epoch: i64,
        seq: i64,
        reader_timestamp: Option<&str>,
        raw_frame: &[u8],
        read_type: &str,
    ) -> Result<(), JournalError> {
        if raw_frame.is_empty() {
            return Err(JournalError::InvalidData(
                "raw_frame must not be empty".to_owned(),
            ));
        }

        self.conn.execute(
            "INSERT INTO events
                 (stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                stream_key,
                seq,
                stream_epoch,
                raw_frame,
                read_type,
                reader_timestamp,
                unix_ms(),
            ],
        )?;
        Ok(())
    }

    /// Update the compatibility ack cursor for the default server receiver.
    pub fn update_ack_cursor(
        &mut self,
        stream_key: &str,
        _acked_epoch: i64,
        acked_through_seq: i64,
    ) -> Result<(), JournalError> {
        self.ensure_compat_receiver()?;
        self.update_receiver_stream_cursor(COMPAT_RECEIVER_ID, stream_key, acked_through_seq)
    }

    /// Update a receiver's cumulative ack cursor for a stream.
    pub fn update_receiver_stream_cursor(
        &mut self,
        endpoint_id: &str,
        stream_key: &str,
        acked_through_seq: i64,
    ) -> Result<(), JournalError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO receivers (endpoint_id, display_name, approved_unix_ms)
             VALUES (?1, ?1, ?2)",
            params![endpoint_id, unix_ms()],
        )?;

        let current_seq = self
            .conn
            .query_row(
                "SELECT acked_through_seq
                 FROM receiver_stream_cursors
                 WHERE endpoint_id = ?1 AND stream_id = ?2",
                params![endpoint_id, stream_key],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);
        if acked_through_seq < current_seq {
            return Ok(());
        }

        self.conn.execute(
            "INSERT INTO receiver_stream_cursors (endpoint_id, stream_id, acked_through_seq)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(endpoint_id, stream_id) DO UPDATE SET
                 acked_through_seq = excluded.acked_through_seq",
            params![endpoint_id, stream_key, acked_through_seq],
        )?;
        Ok(())
    }

    /// Return the compatibility `(acked_epoch, acked_through_seq)` cursor.
    pub fn ack_cursor(&self, stream_key: &str) -> Result<(i64, i64), JournalError> {
        let acked_seq = self
            .conn
            .query_row(
                "SELECT acked_through_seq
                 FROM receiver_stream_cursors
                 WHERE endpoint_id = ?1 AND stream_id = ?2",
                params![COMPAT_RECEIVER_ID, stream_key],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);

        if acked_seq == 0 {
            return Ok((0, 0));
        }

        let acked_epoch = self
            .conn
            .query_row(
                "SELECT epoch
                 FROM events
                 WHERE stream_id = ?1 AND seq <= ?2
                 ORDER BY seq DESC
                 LIMIT 1",
                params![stream_key, acked_seq],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
            .unwrap_or(0);

        Ok((acked_epoch, acked_seq))
    }

    pub fn latest_committed_seq(&self, stream_key: &str) -> Result<i64, JournalError> {
        latest_committed_seq(&self.conn, stream_key)
    }

    pub fn read_events_after(
        &self,
        stream_key: &str,
        after_seq: i64,
        max: usize,
    ) -> Result<Vec<JournalEvent>, JournalError> {
        read_events_after(&self.conn, stream_key, after_seq, max)
    }

    /// Return all unacked events for a stream epoch after `after_seq`.
    pub fn unacked_events(
        &self,
        stream_key: &str,
        stream_epoch: i64,
        after_seq: i64,
    ) -> Result<Vec<JournalEvent>, JournalError> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid, stream_id, epoch, seq, reader_timestamp, raw_frame, read_kind,
                    CAST(received_unix_ms AS TEXT)
             FROM events
             WHERE stream_id = ?1 AND epoch = ?2 AND seq > ?3
             ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![stream_key, stream_epoch, after_seq], map_event)?;
        collect_events(rows)
    }

    /// Count events for a `(stream_key, stream_epoch)` pair.
    pub fn count_events_for_epoch(
        &self,
        stream_key: &str,
        stream_epoch: i64,
    ) -> Result<i64, JournalError> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE stream_id = ?1 AND epoch = ?2",
                params![stream_key, stream_epoch],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Count total events for a stream key.
    pub fn event_count(&self, stream_key: &str) -> Result<i64, JournalError> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE stream_id = ?1",
                params![stream_key],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    /// Count total events across all streams.
    pub fn total_event_count(&self) -> Result<i64, JournalError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .map_err(Into::into)
    }

    /// Delete up to `limit` acked events for `stream_key`.
    pub fn prune_acked(&mut self, stream_key: &str, limit: i64) -> Result<i64, JournalError> {
        let (_, acked_seq) = self.ack_cursor(stream_key)?;
        let deleted = self.conn.execute(
            "DELETE FROM events
             WHERE rowid IN (
                 SELECT rowid FROM events
                 WHERE stream_id = ?1 AND seq <= ?2
                 ORDER BY seq ASC
                 LIMIT ?3
             )",
            params![stream_key, acked_seq, limit],
        )?;
        if deleted > 0 {
            self.conn.execute(
                "UPDATE stream_retention
                 SET earliest_available_seq = COALESCE(
                     (SELECT MIN(seq) FROM events WHERE stream_id = ?1),
                     ?2 + 1
                 )
                 WHERE stream_id = ?1",
                params![stream_key, acked_seq],
            )?;
        }
        Ok(deleted as i64)
    }

    pub fn prune_retention(
        &mut self,
        policy: &RetentionPolicy,
        context: RetentionContext,
    ) -> Result<RetentionPruneStats, JournalError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stats = RetentionPruneStats::default();
        let min_cutoff = context.now_unix_ms.saturating_sub(policy.min_retention_ms);
        let max_cutoff = context.now_unix_ms.saturating_sub(policy.max_retention_ms);

        let acked = retention_candidates(
            &tx,
            "e.received_unix_ms < ?1 AND e.seq <= COALESCE((SELECT MIN(c.acked_through_seq) FROM receiver_stream_cursors c WHERE c.stream_id = e.stream_id), 0)",
            &[&min_cutoff],
            Some(MAX_PRUNE_BATCH),
            false,
        )?;
        let (deleted, forced) = delete_candidates(&tx, &acked)?;
        stats.acked_deleted = deleted;
        stats.forced_gap_count += forced;

        let hard_cap = retention_candidates(
            &tx,
            "e.received_unix_ms < ?1 AND e.seq > COALESCE((SELECT MIN(c.acked_through_seq) FROM receiver_stream_cursors c WHERE c.stream_id = e.stream_id), 0)",
            &[&max_cutoff],
            Some(MAX_PRUNE_BATCH),
            true,
        )?;
        let (deleted, forced) = delete_candidates(&tx, &hard_cap)?;
        stats.hard_cap_deleted = deleted;
        stats.forced_gap_count += forced;

        let total_rows = tx.query_row("SELECT COUNT(*) FROM events", [], |row| {
            row.get::<_, i64>(0)
        })?;
        let emergency_triggered = context.free_disk_bytes < policy.emergency_free_disk_bytes
            || total_rows > policy.emergency_max_rows;
        if emergency_triggered {
            let delete_limit = if total_rows > policy.emergency_max_rows {
                total_rows - policy.emergency_max_rows
            } else {
                1
            };
            let emergency = retention_candidates(
                &tx,
                "e.received_unix_ms < ?1",
                &[&min_cutoff],
                Some(delete_limit.min(MAX_PRUNE_BATCH)),
                true,
            )?;
            let (deleted, forced) = delete_candidates(&tx, &emergency)?;
            stats.emergency_deleted = deleted;
            stats.forced_gap_count += forced;
        }

        tx.commit()?;
        Ok(stats)
    }

    pub fn retention_state(&self, stream_key: &str) -> Result<RetentionState, JournalError> {
        retention_state(&self.conn, stream_key)
    }

    pub fn clear_stream(&mut self, stream_key: &str) -> Result<(), JournalError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current_epoch = current_epoch(&tx, stream_key)?;
        let next_seq = next_seq(&tx, stream_key)?;
        let next_epoch = current_epoch + 1;

        tx.execute(
            "DELETE FROM events WHERE stream_id = ?1",
            params![stream_key],
        )?;
        tx.execute(
            "UPDATE stream_epochs
             SET end_seq = COALESCE(end_seq, ?2 - 1)
             WHERE stream_id = ?1 AND end_seq IS NULL",
            params![stream_key, next_seq],
        )?;
        tx.execute(
            "INSERT INTO stream_epochs
                 (stream_id, epoch, start_seq, end_seq, reason, created_unix_ms)
             VALUES (?1, ?2, ?3, NULL, 'manual_clear', ?4)",
            params![stream_key, next_epoch, next_seq, unix_ms()],
        )?;
        tx.execute(
            "UPDATE stream_retention
             SET earliest_available_seq = MAX(earliest_available_seq, ?2)
             WHERE stream_id = ?1",
            params![stream_key, next_seq],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn current_epoch(&self, stream_key: &str) -> Result<i64, JournalError> {
        current_epoch(&self.conn, stream_key)
    }

    pub fn current_epoch_metadata(
        &self,
        stream_key: &str,
    ) -> Result<Option<CurrentEpochMetadata>, JournalError> {
        current_epoch_metadata(&self.conn, stream_key)
    }

    pub fn epoch_summaries(&self, stream_key: &str) -> Result<Vec<EpochSummary>, JournalError> {
        epoch_summaries(&self.conn, stream_key)
    }

    /// Whether the journal has any stream state for `stream_id`.
    ///
    /// Startup restore uses this to detect journal loss for configured reader
    /// stream keys before reader tasks or the P2P catalog seed them.
    pub fn stream_exists(&self, stream_id: &str) -> Result<bool, JournalError> {
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM streams WHERE stream_id = ?1)",
                params![stream_id],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    fn ensure_compat_receiver(&mut self) -> Result<(), JournalError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO receivers (endpoint_id, display_name, approved_unix_ms)
             VALUES (?1, 'Forwarder server', ?2)",
            params![COMPAT_RECEIVER_ID, unix_ms()],
        )?;
        Ok(())
    }
}

impl ReplayJournal for Journal {
    fn retention_state(&self, stream_key: &str) -> Result<RetentionState, JournalError> {
        self.retention_state(stream_key)
    }

    fn latest_committed_seq(&self, stream_key: &str) -> Result<i64, JournalError> {
        self.latest_committed_seq(stream_key)
    }

    fn read_events_after(
        &self,
        stream_key: &str,
        after_seq: i64,
        max: usize,
    ) -> Result<Vec<JournalEvent>, JournalError> {
        self.read_events_after(stream_key, after_seq, max)
    }
}

fn validate_stream_seed(stream_id: &str, epoch: i64, next_seq: i64) -> Result<(), JournalError> {
    if stream_id.is_empty() {
        return Err(JournalError::InvalidData(
            "stream_id must not be empty".to_owned(),
        ));
    }
    if epoch < 1 {
        return Err(JournalError::InvalidData(format!(
            "epoch {epoch} must be at least 1"
        )));
    }
    if next_seq < 1 {
        return Err(JournalError::InvalidData(format!(
            "next_seq {next_seq} must be at least 1"
        )));
    }
    Ok(())
}

fn insert_stream_seed(
    conn: &Connection,
    stream_id: &str,
    hardware_reader_id: &str,
    epoch: i64,
    next_seq: i64,
    reason: &str,
) -> Result<(), JournalError> {
    let now_ms = unix_ms();
    conn.execute(
        "INSERT OR IGNORE INTO streams
             (stream_id, hardware_reader_id, network_addr, display_name, reader_connected, created_unix_ms)
         VALUES (?1, ?2, ?2, ?2, 0, ?3)",
        params![stream_id, hardware_reader_id, now_ms],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO stream_epochs
             (stream_id, epoch, start_seq, end_seq, reason, created_unix_ms)
         VALUES (?1, ?2, ?3, NULL, ?4, ?5)",
        params![stream_id, epoch, next_seq, reason, now_ms],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO stream_retention
             (stream_id, earliest_available_seq, forced_gap_count)
         VALUES (?1, ?2, 0)",
        params![stream_id, next_seq],
    )?;
    Ok(())
}

fn retention_candidates(
    conn: &Connection,
    predicate: &str,
    predicate_params: &[&dyn rusqlite::ToSql],
    limit: Option<i64>,
    forced: bool,
) -> Result<Vec<PruneCandidate>, JournalError> {
    let sql = if limit.is_some() {
        format!(
            "SELECT e.stream_id, e.seq
             FROM events e
             WHERE {predicate}
             ORDER BY e.seq ASC
             LIMIT ?{}",
            predicate_params.len() + 1
        )
    } else {
        format!(
            "SELECT e.stream_id, e.seq
             FROM events e
             WHERE {predicate}
             ORDER BY e.seq ASC"
        )
    };

    let mut params: Vec<&dyn rusqlite::ToSql> = predicate_params.to_vec();
    if let Some(ref limit) = limit {
        params.push(limit);
    }

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params.as_slice(), |row| {
        Ok(PruneCandidate {
            stream_id: row.get(0)?,
            seq: row.get(1)?,
            forced,
        })
    })?;

    let mut candidates = Vec::new();
    for row in rows {
        candidates.push(row?);
    }
    Ok(candidates)
}

/// Delete candidate rows, but only as a contiguous prefix from each stream's
/// current `earliest_available_seq`.
///
/// Pruning is restricted to a contiguous prefix per stream so the seq
/// high-water is never lost. If we deleted a non-prefix row (a higher seq while
/// a lower seq remained), `MIN(seq)` — and therefore `earliest_available_seq` —
/// would stay low while `MAX(seq)` of the live rows dropped below an
/// already-issued seq, letting `next_seq` reuse it. By only deleting from the
/// bottom upward, the remaining live rows always retain the true maximum seq,
/// so `next_seq` stays monotonic. Non-prefix candidates are left in place and
/// become eligible on a later pass once the floor has advanced.
fn delete_candidates(
    conn: &Connection,
    candidates: &[PruneCandidate],
) -> Result<(i64, i64), JournalError> {
    use std::collections::BTreeMap;

    // Group candidate (seq, forced) pairs by stream so we can evaluate each
    // stream's contiguous prefix independently.
    let mut by_stream: BTreeMap<&str, Vec<(i64, bool)>> = BTreeMap::new();
    for candidate in candidates {
        by_stream
            .entry(candidate.stream_id.as_str())
            .or_default()
            .push((candidate.seq, candidate.forced));
    }

    let mut deleted = 0;
    let mut forced = 0;
    for (stream_id, mut seqs) in by_stream {
        seqs.sort_unstable_by_key(|(seq, _)| *seq);

        let floor: i64 = conn.query_row(
            "SELECT earliest_available_seq FROM stream_retention WHERE stream_id = ?1",
            params![stream_id],
            |row| row.get(0),
        )?;

        let mut deleted_here = 0_i64;
        let mut forced_here = 0_i64;
        let mut last_deleted = floor - 1;
        for (seq, is_forced) in seqs {
            // Stop at the first non-contiguous candidate: deleting beyond a gap
            // would leave a lower seq behind and drop the live high-water.
            let expected = floor + deleted_here;
            if seq != expected {
                break;
            }
            let changed = conn.execute(
                "DELETE FROM events WHERE stream_id = ?1 AND seq = ?2",
                params![stream_id, seq],
            )?;
            if changed == 0 {
                break;
            }
            deleted_here += 1;
            if is_forced {
                forced_here += 1;
            }
            last_deleted = seq;
        }

        if deleted_here > 0 {
            conn.execute(
                "UPDATE stream_retention
                 SET earliest_available_seq = COALESCE(
                         (SELECT MIN(seq) FROM events WHERE stream_id = ?1),
                         MAX(earliest_available_seq, ?2 + 1)
                     ),
                     forced_gap_count = forced_gap_count + ?3
                 WHERE stream_id = ?1",
                params![stream_id, last_deleted, forced_here],
            )?;
        }
        deleted += deleted_here;
        forced += forced_here;
    }
    Ok((deleted, forced))
}

fn apply_read_pragmas(conn: &Connection) -> Result<(), JournalError> {
    // No journal_mode here: the writer sets WAL, and a read-only connection
    // cannot change it. wal_autocheckpoint is also omitted: read-only
    // connections never commit, so they never trigger autocheckpoints.
    conn.execute_batch(
        "PRAGMA query_only=ON;
         PRAGMA busy_timeout=5000;
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(())
}

fn retention_state(conn: &Connection, stream_key: &str) -> Result<RetentionState, JournalError> {
    conn.query_row(
        "SELECT earliest_available_seq, forced_gap_count
         FROM stream_retention
         WHERE stream_id = ?1",
        params![stream_key],
        |row| {
            Ok(RetentionState {
                earliest_available_seq: row.get(0)?,
                forced_gap_count: row.get(1)?,
            })
        },
    )
    .map_err(Into::into)
}

fn latest_committed_seq(conn: &Connection, stream_key: &str) -> Result<i64, JournalError> {
    Ok(next_seq(conn, stream_key)?.saturating_sub(1))
}

fn read_events_after(
    conn: &Connection,
    stream_key: &str,
    after_seq: i64,
    max: usize,
) -> Result<Vec<JournalEvent>, JournalError> {
    let limit = i64::try_from(max).unwrap_or(i64::MAX);
    let mut stmt = conn.prepare(
        "SELECT rowid, stream_id, epoch, seq, reader_timestamp, raw_frame, read_kind,
                CAST(received_unix_ms AS TEXT)
         FROM events
         WHERE stream_id = ?1 AND seq > ?2
         ORDER BY seq ASC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![stream_key, after_seq, limit], map_event)?;
    collect_events(rows)
}

/// Return the next stream-wide sequence number.
///
/// The seq is stream-wide and never resets across epochs. It is derived from
/// durable high-water evidence so that pruning acked events can never cause a
/// previously-issued seq to be reused: we take the maximum of the highest live
/// event seq, the highest pruned seq (recorded via
/// `stream_retention.earliest_available_seq`), and the highest acked seq.
fn next_seq(conn: &Connection, stream_key: &str) -> Result<i64, JournalError> {
    conn.query_row(
        "SELECT MAX(hw) + 1 FROM (
             SELECT COALESCE(
                 (SELECT MAX(seq) FROM events WHERE stream_id = ?1), 0
             ) AS hw
             UNION ALL
             SELECT COALESCE(
                 (SELECT earliest_available_seq - 1 FROM stream_retention WHERE stream_id = ?1),
                 0
             )
             UNION ALL
             SELECT COALESCE(
                 (SELECT MAX(acked_through_seq) FROM receiver_stream_cursors WHERE stream_id = ?1),
                 0
             )
         )",
        params![stream_key],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn current_epoch(conn: &Connection, stream_key: &str) -> Result<i64, JournalError> {
    conn.query_row(
        "SELECT epoch
         FROM stream_epochs
         WHERE stream_id = ?1
         ORDER BY epoch DESC
         LIMIT 1",
        params![stream_key],
        |row| row.get(0),
    )
    .map_err(Into::into)
}

fn epoch_summaries(conn: &Connection, stream_key: &str) -> Result<Vec<EpochSummary>, JournalError> {
    let mut stmt = conn.prepare(
        "SELECT epoch, created_unix_ms
         FROM stream_epochs
         WHERE stream_id = ?1
         ORDER BY epoch DESC",
    )?;
    let rows = stmt.query_map(params![stream_key], |row| {
        Ok(EpochSummary {
            epoch: row.get(0)?,
            created_unix_ms: row.get(1)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn current_epoch_metadata(
    conn: &Connection,
    stream_key: &str,
) -> Result<Option<CurrentEpochMetadata>, JournalError> {
    conn.query_row(
        "SELECT epoch, created_unix_ms
         FROM stream_epochs
         WHERE stream_id = ?1
         ORDER BY epoch DESC
         LIMIT 1",
        params![stream_key],
        |row| {
            Ok(CurrentEpochMetadata {
                epoch: row.get(0)?,
                created_unix_ms: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(Into::into)
}

fn collect_events<F>(rows: rusqlite::MappedRows<'_, F>) -> Result<Vec<JournalEvent>, JournalError>
where
    F: FnMut(&rusqlite::Row<'_>) -> Result<JournalEvent, rusqlite::Error>,
{
    let mut events = Vec::new();
    for row in rows {
        events.push(row?);
    }
    Ok(events)
}

fn map_event(row: &rusqlite::Row<'_>) -> Result<JournalEvent, rusqlite::Error> {
    Ok(JournalEvent {
        id: row.get(0)?,
        stream_key: row.get(1)?,
        stream_epoch: row.get(2)?,
        seq: row.get(3)?,
        reader_timestamp: row.get(4)?,
        raw_frame: row.get(5)?,
        read_type: row.get(6)?,
        received_at: row.get(7)?,
    })
}

fn unix_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{Journal, TransactionBehavior, params};

    #[test]
    fn read_only_journal_sees_committed_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("journal.db");
        let mut journal = Journal::open(&path).expect("open write journal");
        journal
            .ensure_stream_state("stream-a", 7)
            .expect("ensure stream");
        journal
            .append_read("stream-a", Some("1234"), b"one", "RAW")
            .expect("append read");

        let read = Journal::open_read_only(&path).expect("open read journal");

        let retention = read.retention_state("stream-a").expect("retention");
        assert_eq!(retention.earliest_available_seq, 1);
        assert_eq!(read.latest_committed_seq("stream-a").expect("latest"), 1);
        let events = read
            .read_events_after("stream-a", 0, 10)
            .expect("read events");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 1);
        assert_eq!(events[0].raw_frame, b"one");
    }

    #[test]
    fn current_epoch_metadata_tracks_created_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("journal.db");
        let mut journal = Journal::open(&path).expect("open write journal");
        let before_initial = super::unix_ms();

        journal
            .ensure_stream_state("stream-a", 7)
            .expect("ensure stream");
        let initial = journal
            .current_epoch_metadata("stream-a")
            .expect("current epoch metadata")
            .expect("stream has metadata");
        assert_eq!(initial.epoch, 7);
        assert!(
            initial
                .created_unix_ms
                .is_some_and(|ts| ts >= before_initial),
            "initial epoch should record when it was created"
        );

        journal.bump_epoch("stream-a", 8).expect("bump epoch");
        let reset = journal
            .current_epoch_metadata("stream-a")
            .expect("current epoch metadata")
            .expect("stream has metadata");
        assert_eq!(reset.epoch, 8);
        assert!(
            reset.created_unix_ms >= initial.created_unix_ms,
            "new epoch should have a creation timestamp at least as new as the initial epoch"
        );
    }

    #[test]
    fn read_only_journal_rejects_raw_sql_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("journal.db");
        let _journal = Journal::open(&path).expect("open write journal");
        let read = Journal::open_read_only(&path).expect("open read journal");

        let result = read.connection_for_test().execute(
            "INSERT INTO receivers (endpoint_id, display_name, approved_unix_ms)
             VALUES ('receiver-a', 'receiver-a', 0)",
            [],
        );

        assert!(result.is_err(), "read-only connection accepted a write");
    }

    #[test]
    fn read_only_batch_read_succeeds_during_in_flight_write_transaction() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("journal.db");
        let mut journal = Journal::open(&path).expect("open write journal");
        journal
            .ensure_stream_state("stream-a", 1)
            .expect("ensure stream");
        journal
            .append_read("stream-a", None, b"committed", "RAW")
            .expect("append committed read");
        let read = Journal::open_read_only(&path).expect("open read journal");
        let busy_timeout: i64 = read
            .connection_for_test()
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy timeout");
        assert_eq!(busy_timeout, 5000);

        let tx = journal
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .expect("begin write tx");
        tx.execute(
            "INSERT INTO events
                 (stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                "stream-a",
                2,
                1,
                b"uncommitted",
                "RAW",
                Option::<&str>::None,
                0
            ],
        )
        .expect("insert uncommitted read");

        let events = read
            .read_events_after("stream-a", 0, 10)
            .expect("batch read during write tx");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].raw_frame, b"committed");

        tx.rollback().expect("rollback write tx");
    }
}
