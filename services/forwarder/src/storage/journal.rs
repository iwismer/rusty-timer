//! Durable SQLite journal for forwarder events.
//!
//! # Schema
//! - `journal`: stores read events with (stream_key, stream_epoch, seq) as unique key.
//! - `stream_state`: tracks current epoch, next_seq, and ack cursor per stream.
//!
//! # SQLite durability settings
//! Applied at open: WAL, synchronous=FULL, wal_autocheckpoint=1000, foreign_keys=ON.
//! PRAGMA integrity_check runs at open; returns error if it fails.
//!
//! # stream_key
//! `stream_key` = `reader_ip` (forwarder_id is implicit since one forwarder = one device).

use rusqlite::{Connection, params};
use std::path::Path;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

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
            JournalError::Sqlite(e) => write!(f, "SQLite error: {}", e),
            JournalError::IntegrityCheckFailed(s) => write!(f, "Integrity check failed: {}", s),
            JournalError::InvalidData(s) => write!(f, "Invalid data: {}", s),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<rusqlite::Error> for JournalError {
    fn from(e: rusqlite::Error) -> Self {
        JournalError::Sqlite(e)
    }
}

// ---------------------------------------------------------------------------
// Journal struct
// ---------------------------------------------------------------------------

/// The durable SQLite journal for a single forwarder instance.
pub struct Journal {
    conn: Connection,
}

impl Journal {
    /// Open (or create) the journal at the given path.
    ///
    /// Applies PRAGMAs, runs `PRAGMA integrity_check`, and creates tables if needed.
    /// Returns `Err` if integrity_check fails.
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        let conn = Connection::open(path)?;
        apply_pragmas(&conn)?;
        run_integrity_check(&conn)?;
        apply_schema(&conn)?;
        Ok(Journal { conn })
    }

    // -----------------------------------------------------------------------
    // Stream state management
    // -----------------------------------------------------------------------

    /// Initialize stream state if it does not exist yet.
    ///
    /// Call this when a new reader is discovered.
    /// If the stream already exists (from a previous run), does nothing.
    pub fn ensure_stream_state(
        &mut self,
        stream_key: &str,
        initial_epoch: i64,
    ) -> Result<(), JournalError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO stream_state
                 (stream_key, stream_epoch, next_seq, acked_epoch, acked_through_seq)
             VALUES (?1, ?2, 1, 0, 0)",
            params![stream_key, initial_epoch],
        )?;
        Ok(())
    }

    /// Allocate and return the next sequence number for a stream.
    ///
    /// Atomically increments `next_seq` in `stream_state` and returns the
    /// value before the increment (i.e., the seq to use for the new event).
    pub fn next_seq(&mut self, stream_key: &str) -> Result<i64, JournalError> {
        // Read current next_seq
        let current: i64 = self.conn.query_row(
            "SELECT next_seq FROM stream_state WHERE stream_key = ?1",
            params![stream_key],
            |row| row.get(0),
        )?;

        // Increment
        self.conn.execute(
            "UPDATE stream_state SET next_seq = next_seq + 1 WHERE stream_key = ?1",
            params![stream_key],
        )?;

        Ok(current)
    }

    /// Bump the stream epoch to `new_epoch` and reset `next_seq` to 1.
    ///
    /// Does NOT delete old-epoch events; they remain replayable until acked.
    pub fn bump_epoch(&mut self, stream_key: &str, new_epoch: i64) -> Result<(), JournalError> {
        self.conn.execute(
            "UPDATE stream_state SET stream_epoch = ?2, next_seq = 1 WHERE stream_key = ?1",
            params![stream_key, new_epoch],
        )?;
        Ok(())
    }

    /// Return the current epoch and next_seq for a stream.
    pub fn current_epoch_and_next_seq(
        &mut self,
        stream_key: &str,
    ) -> Result<(i64, i64), JournalError> {
        let (epoch, next_seq) = self.conn.query_row(
            "SELECT stream_epoch, next_seq FROM stream_state WHERE stream_key = ?1",
            params![stream_key],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        Ok((epoch, next_seq))
    }

    // -----------------------------------------------------------------------
    // Event persistence
    // -----------------------------------------------------------------------

    /// Insert a read event into the journal.
    ///
    /// `raw_frame` must be non-empty.
    pub fn insert_event(
        &mut self,
        stream_key: &str,
        stream_epoch: i64,
        seq: i64,
        reader_timestamp: Option<&str>,
        raw_frame: &[u8],
        read_type: &str,
    ) -> Result<(), JournalError> {
        // Enforce non-empty raw_frame (guards against callers passing garbage)
        if raw_frame.is_empty() {
            return Err(JournalError::InvalidData(
                "raw_frame must not be empty".to_owned(),
            ));
        }

        let received_at = chrono_now_utc();

        self.conn.execute(
            "INSERT INTO journal
                 (stream_key, stream_epoch, seq, reader_timestamp, raw_frame, read_type, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                stream_key,
                stream_epoch,
                seq,
                reader_timestamp,
                raw_frame,
                read_type,
                received_at,
            ],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Ack cursor
    // -----------------------------------------------------------------------

    /// Update the ack cursor for a stream to `acked_through_seq` in `acked_epoch`.
    ///
    /// The ack cursor records the highest seq the server has acknowledged
    /// for a given epoch. Used to compute the replay starting point after reconnect.
    pub fn update_ack_cursor(
        &mut self,
        stream_key: &str,
        acked_epoch: i64,
        acked_through_seq: i64,
    ) -> Result<(), JournalError> {
        let (current_epoch, current_seq) = self.ack_cursor(stream_key)?;
        let is_stale = acked_epoch < current_epoch
            || (acked_epoch == current_epoch && acked_through_seq < current_seq);
        if is_stale {
            return Ok(());
        }

        self.conn.execute(
            "UPDATE stream_state
             SET acked_epoch = ?2, acked_through_seq = ?3
             WHERE stream_key = ?1",
            params![stream_key, acked_epoch, acked_through_seq],
        )?;
        Ok(())
    }

    /// Return the (acked_epoch, acked_through_seq) cursor for a stream.
    pub fn ack_cursor(&self, stream_key: &str) -> Result<(i64, i64), JournalError> {
        let (epoch, seq) = self.conn.query_row(
            "SELECT acked_epoch, acked_through_seq FROM stream_state WHERE stream_key = ?1",
            params![stream_key],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )?;
        Ok((epoch, seq))
    }

    // -----------------------------------------------------------------------
    // Query helpers
    // -----------------------------------------------------------------------

    /// Return all unacked events for a stream epoch starting after `after_seq`.
    ///
    /// Used by the uplink replay loop to find events that need to be sent/resent.
    pub fn unacked_events(
        &self,
        stream_key: &str,
        stream_epoch: i64,
        after_seq: i64,
    ) -> Result<Vec<JournalEvent>, JournalError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, stream_key, stream_epoch, seq, reader_timestamp, raw_frame, read_type, received_at
             FROM journal
             WHERE stream_key = ?1 AND stream_epoch = ?2 AND seq > ?3
             ORDER BY seq ASC",
        )?;
        let rows = stmt.query_map(params![stream_key, stream_epoch, after_seq], map_event)?;
        let mut events = Vec::new();
        for r in rows {
            events.push(r?);
        }
        Ok(events)
    }

    /// Count events for a (stream_key, stream_epoch) pair.
    pub fn count_events_for_epoch(
        &self,
        stream_key: &str,
        stream_epoch: i64,
    ) -> Result<i64, JournalError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM journal WHERE stream_key = ?1 AND stream_epoch = ?2",
            params![stream_key, stream_epoch],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Count total events for a stream_key (across all epochs).
    pub fn event_count(&self, stream_key: &str) -> Result<i64, JournalError> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM journal WHERE stream_key = ?1",
            params![stream_key],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Count total events in the journal (across all streams and epochs).
    pub fn total_event_count(&self) -> Result<i64, JournalError> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM journal", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Return all events for stream_key with epoch strictly greater than `after_epoch`.
    ///
    /// Used by the replay engine to find events in newer epochs after the ack cursor epoch.
    pub fn unacked_events_across_epochs(
        &self,
        stream_key: &str,
        after_epoch: i64,
    ) -> Result<Vec<JournalEvent>, JournalError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, stream_key, stream_epoch, seq, reader_timestamp, raw_frame, read_type, received_at
             FROM journal
             WHERE stream_key = ?1 AND stream_epoch > ?2
             ORDER BY stream_epoch ASC, seq ASC",
        )?;
        let rows = stmt.query_map(params![stream_key, after_epoch], map_event)?;
        let mut events = Vec::new();
        for r in rows {
            events.push(r?);
        }
        Ok(events)
    }

    // -----------------------------------------------------------------------
    // Pruning
    // -----------------------------------------------------------------------

    /// Delete up to `limit` acked events for `stream_key` (oldest first).
    ///
    /// Pruning priority: acked events first; unacked events are only pruned
    /// when no acked events remain (handled by the caller with `prune_unacked`).
    ///
    /// Returns the number of rows deleted.
    pub fn prune_acked(&mut self, stream_key: &str, limit: i64) -> Result<i64, JournalError> {
        // Get acked_epoch and acked_through_seq from stream_state
        let (acked_epoch, acked_seq) = self.ack_cursor(stream_key)?;

        let deleted = self.conn.execute(
            "DELETE FROM journal
             WHERE stream_key = ?1
               AND id IN (
                   SELECT id FROM journal
                   WHERE stream_key = ?1
                     AND (stream_epoch < ?2
                          OR (stream_epoch = ?2 AND seq <= ?3))
                   ORDER BY id ASC
                   LIMIT ?4
               )",
            params![stream_key, acked_epoch, acked_seq, limit],
        )?;
        Ok(deleted as i64)
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn apply_pragmas(conn: &Connection) -> Result<(), JournalError> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         PRAGMA wal_autocheckpoint=1000;
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(())
}

fn run_integrity_check(conn: &Connection) -> Result<(), JournalError> {
    let result: String = conn.pragma_query_value(None, "integrity_check", |row| row.get(0))?;
    if result != "ok" {
        return Err(JournalError::IntegrityCheckFailed(result));
    }
    Ok(())
}

fn apply_schema(conn: &Connection) -> Result<(), JournalError> {
    conn.execute_batch(include_str!("schema.sql"))?;
    Ok(())
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

/// Simple UTC timestamp string for `received_at` field.
fn chrono_now_utc() -> String {
    // Use std::time since we don't want to add chrono as a direct dep here.
    // Format: ISO 8601 UTC without sub-second precision for simplicity.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Convert to human-readable UTC string (year-month-day T hour:min:sec Z)
    // Using a simple calculation:
    let s = secs;
    let (y, mo, d, h, mi, se) = epoch_to_ymdhms(s);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z", y, mo, d, h, mi, se)
}

fn epoch_to_ymdhms(secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let se = (secs % 60) as u32;
    let mins = secs / 60;
    let mi = (mins % 60) as u32;
    let hours = mins / 60;
    let h = (hours % 24) as u32;
    let days = hours / 24;

    // Gregorian calendar conversion
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if mo <= 2 { y + 1 } else { y } as u32;

    (y, mo, d, h, mi, se)
}
