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

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rusqlite::Connection;
use tracing::{info, warn};

use crate::db::DbError;
use crate::p2p_session::{DurableBatch, EventFact};

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
    /// True when the wire record carried a non-zero `received_unix_ms` (it is
    /// then part of the immutable payload for duplicate-conflict checks);
    /// false when the receiver supplied a local default.
    pub received_unix_ms_explicit: bool,
    /// Chip id parsed once by the session at prepare time.
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

/// Writer tuning knobs. Defaults target the old-hardware deployment: a short
/// commit window groups concurrent streams' batches into one fsync without
/// adding visible latency.
#[derive(Clone, Debug)]
pub struct WriterConfig {
    /// How long the writer keeps draining commands into one transaction after
    /// the first command arrives. Env override: `RT_RECEIVER_COMMIT_WINDOW_MS`.
    pub commit_window: Duration,
    /// Upper bound on records per transaction. Env override:
    /// `RT_RECEIVER_MAX_TX_RECORDS`.
    pub max_records_per_tx: usize,
    /// Test-only: when set, COMMITs abort (via sqlite commit hook) while the
    /// flag is true, exercising the commit-failure path.
    #[cfg(test)]
    pub fail_commits: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl Default for WriterConfig {
    fn default() -> Self {
        Self {
            commit_window: Duration::from_millis(5),
            max_records_per_tx: 2048,
            #[cfg(test)]
            fail_commits: None,
        }
    }
}

impl WriterConfig {
    /// Defaults with environment overrides applied.
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Some(ms) = std::env::var("RT_RECEIVER_COMMIT_WINDOW_MS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
        {
            config.commit_window = Duration::from_millis(ms);
        }
        if let Some(n) = std::env::var("RT_RECEIVER_MAX_TX_RECORDS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
        {
            config.max_records_per_tx = n.max(1);
        }
        config
    }
}

/// Cloneable handle for sending commands to the writer thread.
#[derive(Clone, Debug)]
pub struct WriterHandle {
    tx: tokio::sync::mpsc::Sender<WriteCommand>,
    commits: Arc<AtomicU64>,
}

impl WriterHandle {
    /// Persist one batch and await durability. `Ok` means the rows and the
    /// cursor advance are committed (fsynced) — the caller may ack.
    pub async fn persist_batch(
        &self,
        stream_id: String,
        records: Vec<PreparedRecord>,
    ) -> Result<DurableBatch, WriteError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WriteCommand::PersistBatch {
                stream_id,
                records,
                reply,
            })
            .await
            .map_err(|_| WriteError::Closed("writer channel closed".to_owned()))?;
        rx.await
            .map_err(|_| WriteError::Closed("writer dropped reply".to_owned()))?
    }

    /// Persist a gap marker + cursor jump and await durability.
    pub async fn persist_gap(
        &self,
        stream_id: String,
        gap: PreparedGap,
    ) -> Result<i64, WriteError> {
        let (reply, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WriteCommand::PersistGap {
                stream_id,
                gap,
                reply,
            })
            .await
            .map_err(|_| WriteError::Closed("writer channel closed".to_owned()))?;
        rx.await
            .map_err(|_| WriteError::Closed("writer dropped reply".to_owned()))?
    }

    /// Request a manual PASSIVE WAL checkpoint.
    pub async fn checkpoint(&self) -> Result<(), WriteError> {
        self.tx
            .send(WriteCommand::Checkpoint)
            .await
            .map_err(|_| WriteError::Closed("writer channel closed".to_owned()))
    }

    /// Total successful group commits (test/diagnostics counter).
    pub fn commit_count(&self) -> u64 {
        self.commits.load(Ordering::Relaxed)
    }
}

/// Spawn the dedicated writer thread against `db_path`.
///
/// The thread owns its own connection (never `spawn_blocking` — fsync must
/// not occupy a tokio worker on 2-core targets). Closing every
/// [`WriterHandle`] shuts the thread down after a final drain + TRUNCATE
/// checkpoint.
pub fn spawn_writer(
    db_path: &Path,
    config: WriterConfig,
) -> Result<(WriterHandle, std::thread::JoinHandle<()>), DbError> {
    let conn = Connection::open(db_path)?;
    conn.execute_batch(
        // Phase 2 pragmas (see Db::apply_pragmas for the synchronous=FULL
        // rationale) with automatic checkpointing disabled: the writer runs
        // its own PASSIVE checkpoints between group commits.
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         PRAGMA wal_autocheckpoint=0;
         PRAGMA foreign_keys=ON;
         PRAGMA busy_timeout=10000;
         PRAGMA cache_size=-16384;
         PRAGMA temp_store=MEMORY;",
    )?;
    let (tx, rx) = tokio::sync::mpsc::channel::<WriteCommand>(64);
    let commits = Arc::new(AtomicU64::new(0));
    let thread_commits = Arc::clone(&commits);
    let handle = std::thread::Builder::new()
        .name("rt-sqlite-writer".to_owned())
        .spawn(move || run_writer(conn, rx, &config, &thread_commits))
        .map_err(DbError::Io)?;
    Ok((WriterHandle { tx, commits }, handle))
}

/// Commits between automatic PASSIVE checkpoints (~2048-record txs → the WAL
/// stays in the low thousands of pages).
const CHECKPOINT_EVERY_COMMITS: u64 = 32;

fn run_writer(
    mut conn: Connection,
    mut rx: tokio::sync::mpsc::Receiver<WriteCommand>,
    config: &WriterConfig,
    commits: &AtomicU64,
) {
    let mut cursors: HashMap<String, CursorState> = HashMap::new();
    let mut commits_since_checkpoint: u64 = 0;
    while let Some(first) = rx.blocking_recv() {
        // Group phase: drain further commands until the commit window closes,
        // the record cap is hit, or the channel is exhausted/closed. tokio's
        // mpsc has no blocking recv-with-timeout, so poll try_recv with short
        // sleeps until the deadline.
        let mut commands = vec![first];
        let mut record_count = commands[0].record_len();
        let deadline = Instant::now() + config.commit_window;
        while record_count < config.max_records_per_tx {
            match rx.try_recv() {
                Ok(cmd) => {
                    record_count += cmd.record_len();
                    commands.push(cmd);
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_micros(500));
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
            }
        }

        let committed = process_group(&mut conn, &mut cursors, commands, commits, config);
        if committed {
            commits_since_checkpoint += 1;
            if commits_since_checkpoint >= CHECKPOINT_EVERY_COMMITS {
                run_checkpoint(&conn, "PASSIVE");
                commits_since_checkpoint = 0;
            }
        }
    }
    // Channel closed and drained: final checkpoint, then exit.
    run_checkpoint(&conn, "TRUNCATE");
    info!("sqlite writer thread stopped");
}

fn run_checkpoint(conn: &Connection, mode: &str) {
    if let Err(e) = conn.execute_batch(&format!("PRAGMA wal_checkpoint({mode});")) {
        warn!(error = %e, mode, "WAL checkpoint failed");
    }
}

impl WriteCommand {
    fn record_len(&self) -> usize {
        match self {
            WriteCommand::PersistBatch { records, .. } => records.len(),
            WriteCommand::PersistGap { .. } | WriteCommand::Checkpoint => 1,
        }
    }
}

/// Per-command outcome staged until the group COMMIT succeeds.
enum StagedReply {
    Batch {
        reply: tokio::sync::oneshot::Sender<Result<DurableBatch, WriteError>>,
        result: Result<DurableBatch, WriteError>,
    },
    Gap {
        reply: tokio::sync::oneshot::Sender<Result<i64, WriteError>>,
        result: Result<i64, WriteError>,
    },
}

impl StagedReply {
    fn send(self) {
        match self {
            StagedReply::Batch { reply, result } => {
                let _ = reply.send(result);
            }
            StagedReply::Gap { reply, result } => {
                let _ = reply.send(result);
            }
        }
    }

    fn send_err(self, message: &str) {
        match self {
            StagedReply::Batch { reply, .. } => {
                let _ = reply.send(Err(WriteError::Closed(message.to_owned())));
            }
            StagedReply::Gap { reply, .. } => {
                let _ = reply.send(Err(WriteError::Closed(message.to_owned())));
            }
        }
    }
}

/// Execute one group of commands in a single `IMMEDIATE` transaction with a
/// SAVEPOINT per command. Returns `true` when the group committed.
///
/// Durability contract (B1): cursor mutations are computed into `staged` and
/// merged into the live `cursors` map **only after COMMIT returns Ok**. On any
/// commit/transaction failure every command gets `Err`, the staged state is
/// discarded, and the touched streams' cursor state is dropped so it is
/// re-initialized from the DB — the acked cursor is always derived from
/// committed rows.
fn process_group(
    conn: &mut Connection,
    cursors: &mut HashMap<String, CursorState>,
    commands: Vec<WriteCommand>,
    commits: &AtomicU64,
    #[allow(unused_variables)] config: &WriterConfig,
) -> bool {
    let mut checkpoint_requested = false;
    let mut staged_cursors: HashMap<String, CursorState> = HashMap::new();
    let mut staged_replies: Vec<StagedReply> = Vec::with_capacity(commands.len());
    let mut touched_streams: BTreeSet<String> = BTreeSet::new();

    let group_result: Result<(), DbError> = (|| {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        for command in commands {
            match command {
                WriteCommand::Checkpoint => checkpoint_requested = true,
                WriteCommand::PersistBatch {
                    stream_id,
                    records,
                    reply,
                } => {
                    touched_streams.insert(stream_id.clone());
                    let mut cursor = staged_cursor(&tx, &staged_cursors, cursors, &stream_id)?;
                    match apply_batch(&tx, &stream_id, &records, &mut cursor) {
                        Ok(facts) => {
                            let result = Ok(DurableBatch {
                                through_seq: cursor.durable_cursor(),
                                inserted: Arc::new(facts),
                            });
                            staged_cursors.insert(stream_id, cursor);
                            staged_replies.push(StagedReply::Batch { reply, result });
                        }
                        Err(CommandError::Conflict { seq }) => {
                            // Only this command's savepoint rolled back; the
                            // rest of the group commits normally.
                            staged_replies.push(StagedReply::Batch {
                                reply,
                                result: Err(WriteError::ConflictingDuplicate { stream_id, seq }),
                            });
                        }
                        Err(CommandError::Db(e)) => {
                            staged_replies.push(StagedReply::Batch {
                                reply,
                                result: Err(WriteError::Closed(String::new())),
                            });
                            return Err(e);
                        }
                    }
                }
                WriteCommand::PersistGap {
                    stream_id,
                    gap,
                    reply,
                } => {
                    touched_streams.insert(stream_id.clone());
                    let mut cursor = staged_cursor(&tx, &staged_cursors, cursors, &stream_id)?;
                    match apply_gap(&tx, &stream_id, &gap, &mut cursor) {
                        Ok(()) => {
                            let result = Ok(cursor.durable_cursor());
                            staged_cursors.insert(stream_id, cursor);
                            staged_replies.push(StagedReply::Gap { reply, result });
                        }
                        Err(CommandError::Conflict { .. }) => unreachable!("gaps cannot conflict"),
                        Err(CommandError::Db(e)) => {
                            staged_replies.push(StagedReply::Gap {
                                reply,
                                result: Err(WriteError::Closed(String::new())),
                            });
                            return Err(e);
                        }
                    }
                }
            }
        }
        // One cursor-table upsert per touched stream, from the staged state.
        for (stream_id, cursor) in &staged_cursors {
            crate::db::jump_stream_cursor_conn(&tx, stream_id, cursor.durable_cursor())?;
        }
        // Test-only injected commit failure (exercises the B1 contract path
        // without patching sqlite itself); dropping `tx` rolls everything back.
        #[cfg(test)]
        if let Some(flag) = &config.fail_commits
            && flag.load(Ordering::SeqCst)
        {
            return Err(DbError::IntegrityCheckFailed(
                "injected commit failure".to_owned(),
            ));
        }
        // The single fsync for the whole group.
        tx.commit()?;
        Ok(())
    })();

    match group_result {
        Ok(()) => {
            // COMMIT succeeded: apply staged cursors to the live map, then
            // release the replies (ack-after-durable).
            cursors.extend(staged_cursors);
            commits.fetch_add(1, Ordering::Relaxed);
            for staged in staged_replies {
                staged.send();
            }
            if checkpoint_requested {
                run_checkpoint(conn, "PASSIVE");
            }
            true
        }
        Err(e) => {
            // Transaction failed (command error or COMMIT failure): nothing
            // persisted. Reply Err to every command in the group — never Ok
            // for a rolled-back row — discard the staged cursor state, and
            // drop the touched streams' live state so it is rebuilt from the
            // DB before the next command.
            warn!(error = %e, "writer group transaction failed; all commands in group get Err");
            for stream_id in &touched_streams {
                cursors.remove(stream_id);
            }
            let message = format!("group transaction failed: {e}");
            for staged in staged_replies {
                staged.send_err(&message);
            }
            false
        }
    }
}

/// The cursor state a command starts from: staged (earlier command in this
/// group), live, or lazily initialized from the DB.
fn staged_cursor(
    conn: &Connection,
    staged: &HashMap<String, CursorState>,
    live: &HashMap<String, CursorState>,
    stream_id: &str,
) -> Result<CursorState, DbError> {
    if let Some(cursor) = staged.get(stream_id).or_else(|| live.get(stream_id)) {
        return Ok(cursor.clone());
    }
    let last_contiguous = crate::db::load_stream_cursor_conn(conn, stream_id)?;
    let mut stmt = conn.prepare_cached(
        "SELECT seq FROM received_events WHERE stream_id = ?1 AND seq > ?2 ORDER BY seq",
    )?;
    let stored = stmt
        .query_map(rusqlite::params![stream_id, last_contiguous], |row| {
            row.get::<_, i64>(0)
        })?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(CursorState::rebuild(last_contiguous, stored))
}

enum CommandError {
    Conflict { seq: i64 },
    Db(DbError),
}

impl From<rusqlite::Error> for CommandError {
    fn from(e: rusqlite::Error) -> Self {
        CommandError::Db(e.into())
    }
}

impl From<DbError> for CommandError {
    fn from(e: DbError) -> Self {
        CommandError::Db(e)
    }
}

/// Run one batch inside its own SAVEPOINT. On a conflicting duplicate the
/// savepoint is rolled back (this command persists nothing) and
/// `CommandError::Conflict` is returned; the caller keeps the group alive.
fn apply_batch(
    tx: &rusqlite::Transaction<'_>,
    stream_id: &str,
    records: &[PreparedRecord],
    cursor: &mut CursorState,
) -> Result<Vec<EventFact>, CommandError> {
    tx.execute_batch("SAVEPOINT cmd")?;
    match apply_batch_inner(tx, stream_id, records, cursor) {
        Ok(facts) => {
            tx.execute_batch("RELEASE SAVEPOINT cmd")?;
            Ok(facts)
        }
        Err(e) => {
            tx.execute_batch("ROLLBACK TO SAVEPOINT cmd; RELEASE SAVEPOINT cmd;")?;
            Err(e)
        }
    }
}

fn apply_batch_inner(
    tx: &rusqlite::Transaction<'_>,
    stream_id: &str,
    records: &[PreparedRecord],
    cursor: &mut CursorState,
) -> Result<Vec<EventFact>, CommandError> {
    let mut facts = Vec::with_capacity(records.len());
    let mut insert = tx.prepare_cached(
        "INSERT INTO received_events
         (stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms, dbf_delivered_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)
         ON CONFLICT (stream_id, seq) DO NOTHING",
    )?;
    for record in records {
        let changed = insert.execute(rusqlite::params![
            stream_id,
            record.seq,
            record.epoch,
            record.raw_frame,
            record.read_kind,
            record.reader_timestamp,
            record.received_unix_ms,
        ])?;
        if changed > 0 {
            facts.push(EventFact {
                seq: record.seq,
                epoch: record.epoch,
                received_unix_ms: record.received_unix_ms,
                chip_id: record.chip_id.clone(),
            });
        } else {
            // Idempotent dedup: the stored payload must match, otherwise the
            // forwarder re-sent a conflicting record under the same seq — a
            // data-integrity violation we must not silently ack past. The
            // received_unix_ms comparison applies only when the wire carried
            // an explicit value (see PreparedRecord::received_unix_ms_explicit).
            let existing = crate::db::load_received_event_conn(tx, stream_id, record.seq)?;
            if let Some(existing) = existing {
                let received_unix_ms_conflicts = record.received_unix_ms_explicit
                    && existing.received_unix_ms != record.received_unix_ms;
                let conflicts = existing.epoch != record.epoch
                    || existing.raw_frame != record.raw_frame
                    || existing.read_kind != record.read_kind
                    || existing.reader_timestamp != record.reader_timestamp
                    || received_unix_ms_conflicts;
                if conflicts {
                    return Err(CommandError::Conflict { seq: record.seq });
                }
            }
        }
        cursor.observe(record.seq);
    }
    Ok(facts)
}

fn apply_gap(
    tx: &rusqlite::Transaction<'_>,
    stream_id: &str,
    gap: &PreparedGap,
    cursor: &mut CursorState,
) -> Result<(), CommandError> {
    crate::db::save_gap_marker_conn(
        tx,
        &crate::db::GapMarkerInsert {
            stream_id,
            requested_after_seq: gap.requested_after_seq,
            earliest_available_seq: gap.earliest_available_seq,
            latest_available_seq: gap.latest_available_seq,
            reason: &gap.reason,
            created_unix_ms: gap.created_unix_ms,
        },
    )?;
    cursor.jump_to(gap.earliest_available_seq.saturating_sub(1));
    Ok(())
}

#[cfg(test)]
mod thread_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// Create a schema-initialized temp-file DB the writer and assertions can
    /// share (an in-memory DB cannot be opened by a second connection).
    fn test_db() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("writer-test.sqlite3");
        drop(crate::db::Db::open(&path).unwrap());
        (dir, path)
    }

    fn rec(seq: i64, raw: &str) -> PreparedRecord {
        PreparedRecord {
            seq,
            epoch: 1,
            raw_frame: raw.as_bytes().to_vec(),
            read_kind: "chip".to_owned(),
            reader_timestamp: None,
            received_unix_ms: 1_700_000_000_000 + seq,
            received_unix_ms_explicit: true,
            chip_id: format!("chip-{seq}"),
        }
    }

    fn read_conn(path: &std::path::Path) -> Connection {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        conn.execute_batch("PRAGMA busy_timeout=10000;").unwrap();
        conn
    }

    fn seqs(conn: &Connection, stream_id: &str) -> Vec<i64> {
        let mut stmt = conn
            .prepare("SELECT seq FROM received_events WHERE stream_id = ?1 ORDER BY seq")
            .unwrap();
        stmt.query_map([stream_id], |row| row.get(0))
            .unwrap()
            .collect::<Result<Vec<i64>, _>>()
            .unwrap()
    }

    fn cursor(conn: &Connection, stream_id: &str) -> i64 {
        crate::db::load_stream_cursor_conn(conn, stream_id).unwrap()
    }

    #[tokio::test]
    async fn ack_reply_only_after_commit() {
        let (_dir, path) = test_db();
        let (writer, thread) = spawn_writer(&path, WriterConfig::default()).unwrap();

        let durable = writer
            .persist_batch("s1".to_owned(), vec![rec(1, "a"), rec(2, "b")])
            .await
            .unwrap();
        assert_eq!(durable.through_seq, 2);
        assert_eq!(durable.inserted.len(), 2);

        // Rows and cursor are visible from a second connection immediately
        // after the reply resolves — the reply is proof of durability.
        let reader = read_conn(&path);
        assert_eq!(seqs(&reader, "s1"), vec![1, 2]);
        assert_eq!(cursor(&reader, "s1"), 2);

        let durable = writer
            .persist_batch("s1".to_owned(), vec![rec(4, "d")])
            .await
            .unwrap();
        assert_eq!(durable.through_seq, 2, "seq 3 missing: cursor holds at 2");
        assert_eq!(seqs(&reader, "s1"), vec![1, 2, 4]);
        assert_eq!(cursor(&reader, "s1"), 2);

        drop(writer);
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn group_commit_batches_multiple_commands() {
        let (_dir, path) = test_db();
        let config = WriterConfig {
            commit_window: Duration::from_millis(200),
            ..WriterConfig::default()
        };
        let (writer, thread) = spawn_writer(&path, config).unwrap();

        let (r1, r2, r3, r4) = tokio::join!(
            writer.persist_batch("s1".to_owned(), vec![rec(1, "a")]),
            writer.persist_batch("s2".to_owned(), vec![rec(1, "a")]),
            writer.persist_batch("s3".to_owned(), vec![rec(1, "a")]),
            writer.persist_batch("s4".to_owned(), vec![rec(1, "a")]),
        );
        for result in [r1, r2, r3, r4] {
            assert_eq!(result.unwrap().through_seq, 1);
        }
        assert!(
            writer.commit_count() < 4,
            "4 concurrent commands must group into fewer commits, got {}",
            writer.commit_count()
        );

        drop(writer);
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn conflicting_duplicate_rolls_back_only_its_command() {
        let (_dir, path) = test_db();
        let config = WriterConfig {
            commit_window: Duration::from_millis(200),
            ..WriterConfig::default()
        };
        let (writer, thread) = spawn_writer(&path, config).unwrap();

        // Interleave two streams in one group: s-bad carries an in-batch
        // conflicting duplicate (same seq, divergent payload); s-good is clean.
        let (good, bad) = tokio::join!(
            writer.persist_batch("s-good".to_owned(), vec![rec(1, "g1"), rec(2, "g2")]),
            writer.persist_batch(
                "s-bad".to_owned(),
                vec![rec(1, "original"), rec(2, "fine"), {
                    let mut tampered = rec(1, "tampered");
                    tampered.chip_id = "tampered".to_owned();
                    tampered
                }],
            ),
        );

        let good = good.unwrap();
        assert_eq!(good.through_seq, 2);
        assert!(
            matches!(bad, Err(WriteError::ConflictingDuplicate { seq: 1, .. })),
            "conflicting command must fail, got {bad:?}"
        );

        let reader = read_conn(&path);
        assert_eq!(
            seqs(&reader, "s-good"),
            vec![1, 2],
            "clean stream committed"
        );
        assert_eq!(cursor(&reader, "s-good"), 2);
        assert!(
            seqs(&reader, "s-bad").is_empty(),
            "conflicting command must persist nothing"
        );
        assert_eq!(cursor(&reader, "s-bad"), 0);

        drop(writer);
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn failed_commit_replies_err_and_preserves_cursor() {
        let (_dir, path) = test_db();
        let fail = Arc::new(AtomicBool::new(true));
        let config = WriterConfig {
            fail_commits: Some(Arc::clone(&fail)),
            ..WriterConfig::default()
        };
        let (writer, thread) = spawn_writer(&path, config).unwrap();

        // While commits abort, every batched command gets Err and nothing is
        // persisted — never reply Ok for a rolled-back row (contract B1).
        let result = writer
            .persist_batch("s1".to_owned(), vec![rec(1, "a"), rec(2, "b")])
            .await;
        assert!(result.is_err(), "commit failure must reply Err");
        assert_eq!(writer.commit_count(), 0);

        let reader = read_conn(&path);
        assert!(
            seqs(&reader, "s1").is_empty(),
            "rolled-back rows must not persist"
        );
        assert_eq!(cursor(&reader, "s1"), 0, "cursor must not advance");

        // Replaying the same batch after the fault clears produces the
        // correct durable cursor (the in-memory state was rebuilt from DB).
        fail.store(false, Ordering::SeqCst);
        let durable = writer
            .persist_batch("s1".to_owned(), vec![rec(1, "a"), rec(2, "b")])
            .await
            .unwrap();
        assert_eq!(durable.through_seq, 2);
        assert_eq!(durable.inserted.len(), 2);
        assert_eq!(seqs(&reader, "s1"), vec![1, 2]);
        assert_eq!(cursor(&reader, "s1"), 2);

        drop(writer);
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn duplicates_excluded_from_facts() {
        let (_dir, path) = test_db();
        let (writer, thread) = spawn_writer(&path, WriterConfig::default()).unwrap();

        let first = writer
            .persist_batch("s1".to_owned(), vec![rec(1, "a"), rec(2, "b")])
            .await
            .unwrap();
        assert_eq!(first.inserted.len(), 2);

        // Benign retransmit: same payloads, no new rows, no facts.
        let second = writer
            .persist_batch("s1".to_owned(), vec![rec(1, "a"), rec(2, "b")])
            .await
            .unwrap();
        assert_eq!(second.through_seq, 2);
        assert!(
            second.inserted.is_empty(),
            "retransmits must yield no facts"
        );

        let reader = read_conn(&path);
        assert_eq!(seqs(&reader, "s1"), vec![1, 2]);

        drop(writer);
        thread.join().unwrap();
    }

    #[tokio::test]
    async fn gap_command_jumps_cursor_and_persists_marker() {
        let (_dir, path) = test_db();
        let (writer, thread) = spawn_writer(&path, WriterConfig::default()).unwrap();

        let cursor_after = writer
            .persist_gap(
                "s1".to_owned(),
                PreparedGap {
                    requested_after_seq: 0,
                    earliest_available_seq: 15,
                    latest_available_seq: 20,
                    reason: "retention-window".to_owned(),
                    created_unix_ms: 1_700_000_000_000,
                },
            )
            .await
            .unwrap();
        assert_eq!(cursor_after, 14);

        let reader = read_conn(&path);
        assert_eq!(cursor(&reader, "s1"), 14);
        let marker_count: i64 = reader
            .query_row(
                "SELECT COUNT(*) FROM gap_markers WHERE stream_id = 's1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(marker_count, 1);

        drop(writer);
        thread.join().unwrap();
    }
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
