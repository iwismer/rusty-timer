/// SQLite durability and schema tests for the forwarder journal.
///
/// Task 6 schema: uses `stream_key` (not `reader_ip`), `stream_state` table,
/// no `acked` column, no `config` table.
///
/// Validates:
/// - WAL journal mode is set correctly
/// - synchronous=FULL is set
/// - Write survives close/reopen cycle
/// - UNIQUE constraint on (stream_key, stream_epoch, seq)
/// - integrity_check passes on a fresh database
/// - Duplicate inserts are rejected (not silently swallowed)
use rusqlite::Connection;
use std::path::Path;

const SCHEMA_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/storage/schema.sql");

/// Helper: open an in-memory database and apply PRAGMAs + schema.
fn open_memory_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory SQLite");
    apply_pragmas(&conn);
    apply_schema(&conn);
    conn
}

/// Helper: open a file-backed database and apply PRAGMAs + schema.
fn open_file_db(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("open file-backed SQLite");
    apply_pragmas(&conn);
    apply_schema(&conn);
    conn
}

/// Helper: reopen a file-backed database and apply PRAGMAs only (schema already exists).
fn reopen_file_db(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("reopen file-backed SQLite");
    apply_pragmas(&conn);
    conn
}

fn apply_pragmas(conn: &Connection) {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         PRAGMA wal_autocheckpoint=1000;
         PRAGMA foreign_keys=ON;",
    )
    .expect("PRAGMAs should succeed");
}

fn apply_schema(conn: &Connection) {
    let schema_sql = std::fs::read_to_string(SCHEMA_PATH)
        .expect("Schema file should exist at services/forwarder/src/storage/schema.sql");
    conn.execute_batch(&schema_sql)
        .expect("Schema SQL should apply without errors");
}

// ---------------------------------------------------------------------------
// PRAGMA tests
// ---------------------------------------------------------------------------

#[test]
fn wal_mode_is_set() {
    // WAL mode requires a file-backed database; in-memory DBs always report "memory".
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("wal_test.db");
    let conn = open_file_db(&db_path);
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("query journal_mode");
    assert_eq!(mode.to_lowercase(), "wal", "journal_mode must be WAL");
}

#[test]
fn synchronous_full_is_set() {
    let conn = open_memory_db();
    let sync_val: i64 = conn
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .expect("query synchronous");
    // synchronous=FULL is value 2
    assert_eq!(sync_val, 2, "synchronous must be FULL (2)");
}

#[test]
fn foreign_keys_enabled() {
    let conn = open_memory_db();
    let fk: i64 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .expect("query foreign_keys");
    assert_eq!(fk, 1, "foreign_keys must be ON (1)");
}

// ---------------------------------------------------------------------------
// Schema validation (Task 6 schema)
// ---------------------------------------------------------------------------

#[test]
fn schema_file_exists_and_is_nonempty() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).expect("Schema file should exist");
    assert!(!sql.trim().is_empty(), "Schema file must not be empty");
}

#[test]
fn schema_creates_journal_table_with_stream_key() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).unwrap();
    assert!(
        sql.contains("CREATE TABLE IF NOT EXISTS journal"),
        "Schema must define journal table"
    );
    assert!(
        sql.contains("stream_key"),
        "Task 6 schema must use stream_key (not reader_ip)"
    );
}

#[test]
fn schema_creates_stream_state_table() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).unwrap();
    assert!(
        sql.contains("CREATE TABLE IF NOT EXISTS stream_state"),
        "Task 6 schema must define stream_state table"
    );
}

// ---------------------------------------------------------------------------
// Integrity check
// ---------------------------------------------------------------------------

#[test]
fn integrity_check_passes_on_fresh_db() {
    let conn = open_memory_db();
    let result: String = conn
        .pragma_query_value(None, "integrity_check", |row| row.get(0))
        .expect("run integrity_check");
    assert_eq!(
        result, "ok",
        "integrity_check must return 'ok' on a fresh database"
    );
}

// ---------------------------------------------------------------------------
// Write durability: write survives close/reopen
// ---------------------------------------------------------------------------

#[test]
fn write_survives_reopen() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("forwarder_test.db");

    // Open, write, close
    {
        let conn = open_file_db(&db_path);
        conn.execute(
            "INSERT INTO journal (stream_key, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, received_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params!["192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01,00:01:23.456", "RAW", "2026-01-01T00:00:00Z"],
        )
        .expect("insert should succeed");
    }

    // Reopen and verify
    {
        let conn = reopen_file_db(&db_path);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM journal", [], |row| row.get(0))
            .expect("count query");
        assert_eq!(count, 1, "Inserted row must survive close/reopen");

        let raw_line: String = conn
            .query_row(
                "SELECT raw_read_line FROM journal WHERE seq = 1",
                [],
                |row| row.get(0),
            )
            .expect("select row");
        assert_eq!(raw_line, "aa01,00:01:23.456");
    }
}

// ---------------------------------------------------------------------------
// UNIQUE constraint on (stream_key, stream_epoch, seq)
// ---------------------------------------------------------------------------

#[test]
fn unique_constraint_rejects_duplicate() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO journal (stream_key, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01,00:01:23.456", "RAW", "2026-01-01T00:00:00Z"],
    )
    .expect("first insert should succeed");

    let result = conn.execute(
        "INSERT INTO journal (stream_key, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01,00:01:23.456", "RAW", "2026-01-01T00:00:00Z"],
    );

    assert!(
        result.is_err(),
        "Duplicate (stream_key, stream_epoch, seq) must be rejected"
    );
}

#[test]
fn unique_constraint_allows_different_seq() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO journal (stream_key, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01", "RAW", "2026-01-01T00:00:00Z"],
    )
    .expect("first insert should succeed");

    conn.execute(
        "INSERT INTO journal (stream_key, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["192.168.1.100", 1, 2, "2026-01-01T00:00:01Z", "aa02", "RAW", "2026-01-01T00:00:01Z"],
    )
    .expect("different seq should be allowed");
}

#[test]
fn unique_constraint_allows_different_epoch() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO journal (stream_key, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01", "RAW", "2026-01-01T00:00:00Z"],
    )
    .expect("first insert should succeed");

    conn.execute(
        "INSERT INTO journal (stream_key, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["192.168.1.100", 2, 1, "2026-01-01T00:00:01Z", "aa02", "RAW", "2026-01-01T00:00:01Z"],
    )
    .expect("same seq but different epoch should be allowed");
}

#[test]
fn unique_constraint_allows_different_stream_key() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO journal (stream_key, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01", "RAW", "2026-01-01T00:00:00Z"],
    )
    .expect("first insert should succeed");

    conn.execute(
        "INSERT INTO journal (stream_key, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, received_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["192.168.1.200", 1, 1, "2026-01-01T00:00:01Z", "aa02", "RAW", "2026-01-01T00:00:01Z"],
    )
    .expect("same seq and epoch but different stream_key should be allowed");
}

// ---------------------------------------------------------------------------
// stream_state table basic operations
// ---------------------------------------------------------------------------

#[test]
fn stream_state_insert_and_read() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO stream_state (stream_key, stream_epoch, next_seq, acked_epoch, acked_through_seq)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params!["192.168.1.100", 1, 1, 0, 0],
    )
    .expect("stream_state insert should succeed");

    let (epoch, next_seq): (i64, i64) = conn
        .query_row(
            "SELECT stream_epoch, next_seq FROM stream_state WHERE stream_key = ?1",
            rusqlite::params!["192.168.1.100"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("stream_state read");
    assert_eq!(epoch, 1);
    assert_eq!(next_seq, 1);
}
