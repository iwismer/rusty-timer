//! Durable SQLite journal for forwarder events.
//!
//! The on-disk schema is the clean-slate P2P schema managed by
//! `storage::migrations`. A few methods retain the old `Journal` API so the
//! existing forwarder code keeps compiling until journal allocation is rewritten.

use crate::storage::migrations;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::path::Path;
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

struct PruneCandidate {
    stream_id: String,
    seq: i64,
    forced: bool,
}

/// A read event retrieved from the journal.
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

/// The durable SQLite journal for a single forwarder instance.
pub struct Journal {
    conn: Connection,
}

impl Journal {
    /// Open (or create) the journal at the given path.
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let conn = Connection::open(path)?;
        migrations::migrate(&conn)?;
        Ok(Journal { conn })
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
                 (stream_id, epoch, start_seq, end_seq, reason)
             VALUES (?1, ?2, 1, NULL, 'initial')",
            params![stream_key, initial_epoch],
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
                 (stream_id, epoch, start_seq, end_seq, reason)
             VALUES (?1, ?2, ?3, NULL, 'reset')",
            params![stream_key, new_epoch, start_seq],
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
        let (_, current_seq) = self.ack_cursor(stream_key)?;
        if acked_through_seq < current_seq {
            return Ok(());
        }

        self.conn.execute(
            "INSERT INTO receiver_stream_cursors (endpoint_id, stream_id, acked_through_seq)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(endpoint_id, stream_id) DO UPDATE SET
                 acked_through_seq = excluded.acked_through_seq",
            params![COMPAT_RECEIVER_ID, stream_key, acked_through_seq],
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

    /// Return all events for stream key with epoch strictly greater than `after_epoch`.
    pub fn unacked_events_across_epochs(
        &self,
        stream_key: &str,
        after_epoch: i64,
    ) -> Result<Vec<JournalEvent>, JournalError> {
        let mut stmt = self.conn.prepare(
            "SELECT rowid, stream_id, epoch, seq, reader_timestamp, raw_frame, read_kind,
                    CAST(received_unix_ms AS TEXT)
             FROM events
             WHERE stream_id = ?1 AND epoch > ?2
             ORDER BY epoch ASC, seq ASC",
        )?;
        let rows = stmt.query_map(params![stream_key, after_epoch], map_event)?;
        collect_events(rows)
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
        self.conn
            .query_row(
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
                 (stream_id, epoch, start_seq, end_seq, reason)
             VALUES (?1, ?2, ?3, NULL, 'manual_clear')",
            params![stream_key, next_epoch, next_seq],
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

    fn ensure_compat_receiver(&mut self) -> Result<(), JournalError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO receivers (endpoint_id, display_name, approved_unix_ms)
             VALUES (?1, 'Forwarder server', ?2)",
            params![COMPAT_RECEIVER_ID, unix_ms()],
        )?;
        Ok(())
    }
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
