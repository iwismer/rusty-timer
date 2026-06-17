use forwarder::storage::migrations::{integrity_check, migrate};
use rusqlite::Connection;
use std::path::Path;

const SCHEMA_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/storage/schema.sql");

fn open_memory_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory SQLite");
    migrate(&conn).expect("migration should succeed");
    conn
}

fn open_file_db(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("open file-backed SQLite");
    migrate(&conn).expect("migration should succeed");
    conn
}

fn insert_stream(conn: &Connection, stream_id: &str, epoch: i64) {
    conn.execute(
        "INSERT INTO streams
             (stream_id, hardware_reader_id, network_addr, display_name, reader_connected, created_unix_ms)
         VALUES (?1, ?1, ?1, ?1, 1, 1760000000000)",
        rusqlite::params![stream_id],
    )
    .expect("stream insert should succeed");
    conn.execute(
        "INSERT INTO stream_epochs (stream_id, epoch, start_seq, end_seq, reason)
         VALUES (?1, ?2, 1, NULL, 'test')",
        rusqlite::params![stream_id, epoch],
    )
    .expect("epoch insert should succeed");
}

#[test]
fn wal_mode_is_set() {
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

#[test]
fn schema_file_exists_and_is_nonempty() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).expect("Schema file should exist");
    assert!(!sql.trim().is_empty(), "Schema file must not be empty");
}

#[test]
fn schema_creates_contract_tables() {
    let conn = open_memory_db();
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .expect("prepare table query");
    let tables = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query tables")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect tables");

    assert_eq!(
        tables,
        vec![
            "events",
            "receiver_stream_cursors",
            "receivers",
            "stream_epochs",
            "stream_retention",
            "streams",
        ]
    );
}

#[test]
fn schema_omits_legacy_journal_tables() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).unwrap();
    assert!(!sql.contains("CREATE TABLE IF NOT EXISTS journal"));
    assert!(!sql.contains("CREATE TABLE IF NOT EXISTS stream_state"));
}

#[test]
fn integrity_check_passes_on_fresh_db() {
    let conn = open_memory_db();
    integrity_check(&conn).expect("integrity_check must pass on a fresh database");
}

#[test]
fn write_survives_reopen() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("forwarder_test.db");

    {
        let conn = open_file_db(&db_path);
        insert_stream(&conn, "stream-1", 1);
        conn.execute(
            "INSERT INTO events (stream_id, seq, epoch, raw_frame, read_kind, reader_timestamp, received_unix_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                "stream-1",
                1,
                1,
                b"aa01,00:01:23.456\r\n".to_vec(),
                "RAW",
                "2026-01-01T00:00:00Z",
                1760000000000_i64
            ],
        )
        .expect("insert should succeed");
    }

    {
        let conn = open_file_db(&db_path);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
            .expect("count query");
        assert_eq!(count, 1, "Inserted row must survive close/reopen");

        let raw_frame: Vec<u8> = conn
            .query_row("SELECT raw_frame FROM events WHERE seq = 1", [], |row| {
                row.get(0)
            })
            .expect("select row");
        assert_eq!(raw_frame, b"aa01,00:01:23.456\r\n".to_vec());
    }
}

#[test]
fn primary_key_rejects_duplicate_stream_seq() {
    let conn = open_memory_db();
    insert_stream(&conn, "stream-1", 1);

    conn.execute(
        "INSERT INTO events (stream_id, seq, epoch, raw_frame, read_kind, received_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["stream-1", 1, 1, b"aa01".to_vec(), "RAW", 1760000000000_i64],
    )
    .expect("first insert should succeed");

    let result = conn.execute(
        "INSERT INTO events (stream_id, seq, epoch, raw_frame, read_kind, received_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["stream-1", 1, 1, b"aa01".to_vec(), "RAW", 1760000000001_i64],
    );

    assert!(
        result.is_err(),
        "Duplicate (stream_id, seq) must be rejected"
    );
}

#[test]
fn primary_key_allows_different_seq() {
    let conn = open_memory_db();
    insert_stream(&conn, "stream-1", 1);

    conn.execute(
        "INSERT INTO events (stream_id, seq, epoch, raw_frame, read_kind, received_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["stream-1", 1, 1, b"aa01".to_vec(), "RAW", 1760000000000_i64],
    )
    .expect("first insert should succeed");

    conn.execute(
        "INSERT INTO events (stream_id, seq, epoch, raw_frame, read_kind, received_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["stream-1", 2, 1, b"aa02".to_vec(), "RAW", 1760000000001_i64],
    )
    .expect("different seq should be allowed");
}

#[test]
fn primary_key_allows_different_stream() {
    let conn = open_memory_db();
    insert_stream(&conn, "stream-1", 1);
    insert_stream(&conn, "stream-2", 1);

    conn.execute(
        "INSERT INTO events (stream_id, seq, epoch, raw_frame, read_kind, received_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["stream-1", 1, 1, b"aa01".to_vec(), "RAW", 1760000000000_i64],
    )
    .expect("first insert should succeed");

    conn.execute(
        "INSERT INTO events (stream_id, seq, epoch, raw_frame, read_kind, received_unix_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params!["stream-2", 1, 1, b"aa02".to_vec(), "RAW", 1760000000001_i64],
    )
    .expect("same seq on another stream should be allowed");
}

#[test]
fn receiver_cursor_insert_and_read() {
    let conn = open_memory_db();
    insert_stream(&conn, "stream-1", 1);
    conn.execute(
        "INSERT INTO receivers (endpoint_id, display_name, approved_unix_ms)
         VALUES (?1, ?2, ?3)",
        rusqlite::params!["receiver-1", "Receiver 1", 1760000000000_i64],
    )
    .expect("receiver insert should succeed");
    conn.execute(
        "INSERT INTO receiver_stream_cursors (endpoint_id, stream_id, acked_through_seq)
         VALUES (?1, ?2, ?3)",
        rusqlite::params!["receiver-1", "stream-1", 12_i64],
    )
    .expect("cursor insert should succeed");

    let acked: i64 = conn
        .query_row(
            "SELECT acked_through_seq FROM receiver_stream_cursors WHERE endpoint_id = ?1 AND stream_id = ?2",
            rusqlite::params!["receiver-1", "stream-1"],
            |row| row.get(0),
        )
        .expect("cursor read");
    assert_eq!(acked, 12);
}
