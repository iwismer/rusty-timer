/// SQLite durability and schema tests for the receiver event cache.
///
/// Validates:
/// - WAL journal mode is set correctly
/// - synchronous=FULL is set
/// - Write survives close/reopen cycle
/// - UNIQUE constraint on (forwarder_id, reader_ip, stream_epoch, seq)
/// - integrity_check passes on a fresh database
/// - Duplicate inserts are rejected (not silently swallowed)
/// - stream_cursors table works correctly

use rusqlite::Connection;
use std::path::Path;

const SCHEMA_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/storage/schema.sql"
);

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
        .expect("Schema file should exist at services/receiver/src/storage/schema.sql");
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
    assert_eq!(
        mode.to_lowercase(),
        "wal",
        "journal_mode must be WAL"
    );
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
// Schema validation
// ---------------------------------------------------------------------------

#[test]
fn schema_file_exists_and_is_nonempty() {
    let sql = std::fs::read_to_string(SCHEMA_PATH)
        .expect("Schema file should exist");
    assert!(!sql.trim().is_empty(), "Schema file must not be empty");
}

#[test]
fn schema_creates_event_cache_table() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).unwrap();
    assert!(
        sql.contains("CREATE TABLE IF NOT EXISTS event_cache"),
        "Schema must define event_cache table"
    );
}

#[test]
fn schema_creates_stream_cursors_table() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).unwrap();
    assert!(
        sql.contains("CREATE TABLE IF NOT EXISTS stream_cursors"),
        "Schema must define stream_cursors table"
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
    let db_path = dir.path().join("receiver_test.db");

    // Open, write, close
    {
        let conn = open_file_db(&db_path);
        conn.execute(
            "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params!["fwd-001", "192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01,00:01:23.456", "RAW"],
        )
        .expect("insert should succeed");
    }

    // Reopen and verify
    {
        let conn = reopen_file_db(&db_path);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM event_cache", [], |row| row.get(0))
            .expect("count query");
        assert_eq!(count, 1, "Inserted row must survive close/reopen");

        let raw_line: String = conn
            .query_row(
                "SELECT raw_read_line FROM event_cache WHERE seq = 1",
                [],
                |row| row.get(0),
            )
            .expect("select row");
        assert_eq!(raw_line, "aa01,00:01:23.456");
    }
}

// ---------------------------------------------------------------------------
// UNIQUE constraint on (forwarder_id, reader_ip, stream_epoch, seq)
// ---------------------------------------------------------------------------

#[test]
fn unique_constraint_rejects_duplicate() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["fwd-001", "192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01,00:01:23.456", "RAW"],
    )
    .expect("first insert should succeed");

    let result = conn.execute(
        "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["fwd-001", "192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01,00:01:23.456", "RAW"],
    );

    assert!(
        result.is_err(),
        "Duplicate (forwarder_id, reader_ip, stream_epoch, seq) must be rejected, not silently swallowed"
    );
}

#[test]
fn unique_constraint_allows_different_seq() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["fwd-001", "192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01,00:01:23.456", "RAW"],
    )
    .expect("first insert should succeed");

    conn.execute(
        "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["fwd-001", "192.168.1.100", 1, 2, "2026-01-01T00:00:01Z", "aa02,00:01:24.567", "RAW"],
    )
    .expect("different seq should be allowed");
}

#[test]
fn unique_constraint_allows_different_epoch() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["fwd-001", "192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01,00:01:23.456", "RAW"],
    )
    .expect("first insert should succeed");

    conn.execute(
        "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["fwd-001", "192.168.1.100", 2, 1, "2026-01-01T00:00:01Z", "aa02,00:01:24.567", "RAW"],
    )
    .expect("same seq but different epoch should be allowed");
}

#[test]
fn unique_constraint_allows_different_forwarder() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["fwd-001", "192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01,00:01:23.456", "RAW"],
    )
    .expect("first insert should succeed");

    conn.execute(
        "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["fwd-002", "192.168.1.100", 1, 1, "2026-01-01T00:00:01Z", "aa02,00:01:24.567", "RAW"],
    )
    .expect("same seq/epoch/ip but different forwarder_id should be allowed");
}

#[test]
fn unique_constraint_allows_different_reader_ip() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["fwd-001", "192.168.1.100", 1, 1, "2026-01-01T00:00:00Z", "aa01,00:01:23.456", "RAW"],
    )
    .expect("first insert should succeed");

    conn.execute(
        "INSERT INTO event_cache (forwarder_id, reader_ip, stream_epoch, seq, reader_timestamp, raw_read_line, read_type)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params!["fwd-001", "192.168.1.200", 1, 1, "2026-01-01T00:00:01Z", "aa02,00:01:24.567", "RAW"],
    )
    .expect("same forwarder/seq/epoch but different reader_ip should be allowed");
}

// ---------------------------------------------------------------------------
// stream_cursors table
// ---------------------------------------------------------------------------

#[test]
fn stream_cursors_insert_and_read() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO stream_cursors (forwarder_id, reader_ip, stream_epoch, last_seq)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params!["fwd-001", "192.168.1.100", 1, 42],
    )
    .expect("cursor insert should succeed");

    let last_seq: i64 = conn
        .query_row(
            "SELECT last_seq FROM stream_cursors WHERE forwarder_id = ?1 AND reader_ip = ?2",
            rusqlite::params!["fwd-001", "192.168.1.100"],
            |row| row.get(0),
        )
        .expect("cursor read");
    assert_eq!(last_seq, 42);
}

#[test]
fn stream_cursors_pk_rejects_duplicate() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO stream_cursors (forwarder_id, reader_ip, stream_epoch, last_seq)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params!["fwd-001", "192.168.1.100", 1, 42],
    )
    .expect("first cursor insert should succeed");

    let result = conn.execute(
        "INSERT INTO stream_cursors (forwarder_id, reader_ip, stream_epoch, last_seq)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params!["fwd-001", "192.168.1.100", 2, 100],
    );

    assert!(
        result.is_err(),
        "Duplicate (forwarder_id, reader_ip) in stream_cursors must be rejected"
    );
}

#[test]
fn stream_cursors_allows_different_forwarder() {
    let conn = open_memory_db();

    conn.execute(
        "INSERT INTO stream_cursors (forwarder_id, reader_ip, stream_epoch, last_seq)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params!["fwd-001", "192.168.1.100", 1, 42],
    )
    .expect("first cursor insert");

    conn.execute(
        "INSERT INTO stream_cursors (forwarder_id, reader_ip, stream_epoch, last_seq)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params!["fwd-002", "192.168.1.100", 1, 10],
    )
    .expect("different forwarder_id should be allowed");
}

// ---------------------------------------------------------------------------
// Durability: cursor survives reopen
// ---------------------------------------------------------------------------

#[test]
fn cursor_survives_reopen() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("receiver_cursor_test.db");

    {
        let conn = open_file_db(&db_path);
        conn.execute(
            "INSERT INTO stream_cursors (forwarder_id, reader_ip, stream_epoch, last_seq)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params!["fwd-001", "192.168.1.100", 1, 99],
        )
        .expect("cursor insert");
    }

    {
        let conn = reopen_file_db(&db_path);
        let last_seq: i64 = conn
            .query_row(
                "SELECT last_seq FROM stream_cursors WHERE forwarder_id = ?1 AND reader_ip = ?2",
                rusqlite::params!["fwd-001", "192.168.1.100"],
                |row| row.get(0),
            )
            .expect("cursor read after reopen");
        assert_eq!(last_seq, 99, "Cursor must survive close/reopen");
    }
}
