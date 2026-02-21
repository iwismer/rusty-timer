/// SQLite durability and schema tests for the receiver (v1 schema).
///
/// Validates:
/// - WAL journal mode is set correctly
/// - synchronous=FULL is set
/// - foreign_keys=ON is set
/// - Schema creates profile, subscriptions, cursors tables (NOT event_cache or stream_cursors)
/// - integrity_check passes on a fresh database
/// - Write survives close/reopen cycle (profile, cursor)
/// - UNIQUE / PRIMARY KEY constraints on cursors and subscriptions
use rusqlite::Connection;
use std::path::Path;

const SCHEMA_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src/storage/schema.sql");

fn open_memory_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory SQLite");
    apply_pragmas(&conn);
    apply_schema(&conn);
    conn
}

fn open_file_db(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("open file-backed SQLite");
    apply_pragmas(&conn);
    apply_schema(&conn);
    conn
}

fn reopen_file_db(path: &Path) -> Connection {
    let conn = Connection::open(path).expect("reopen file-backed SQLite");
    apply_pragmas(&conn);
    conn
}

fn apply_pragmas(conn: &Connection) {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA wal_autocheckpoint=1000; PRAGMA foreign_keys=ON;",
    ).expect("PRAGMAs should succeed");
}

fn apply_schema(conn: &Connection) {
    let sql = std::fs::read_to_string(SCHEMA_PATH).expect("schema file must exist");
    conn.execute_batch(&sql)
        .expect("schema SQL should apply without errors");
}

// ---------------------------------------------------------------------------
// PRAGMA tests
// ---------------------------------------------------------------------------

#[test]
fn wal_mode_is_set() {
    let dir = tempfile::tempdir().expect("create temp dir");
    let db_path = dir.path().join("wal_test.db");
    let conn = open_file_db(&db_path);
    let mode: String = conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(mode.to_lowercase(), "wal", "journal_mode must be WAL");
}

#[test]
fn synchronous_full_is_set() {
    let conn = open_memory_db();
    let v: i64 = conn
        .pragma_query_value(None, "synchronous", |r| r.get(0))
        .unwrap();
    assert_eq!(v, 2, "synchronous must be FULL (2)");
}

#[test]
fn foreign_keys_enabled() {
    let conn = open_memory_db();
    let v: i64 = conn
        .pragma_query_value(None, "foreign_keys", |r| r.get(0))
        .unwrap();
    assert_eq!(v, 1, "foreign_keys must be ON (1)");
}

// ---------------------------------------------------------------------------
// Schema validation - v1 tables
// ---------------------------------------------------------------------------

#[test]
fn schema_file_exists_and_is_nonempty() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).expect("schema file must exist");
    assert!(!sql.trim().is_empty(), "schema file must not be empty");
}

#[test]
fn schema_creates_profile_table() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).unwrap();
    assert!(
        sql.contains("CREATE TABLE IF NOT EXISTS profile"),
        "schema must define profile table"
    );
}

#[test]
fn schema_creates_subscriptions_table() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).unwrap();
    assert!(
        sql.contains("CREATE TABLE IF NOT EXISTS subscriptions"),
        "schema must define subscriptions table"
    );
}

#[test]
fn schema_creates_cursors_table() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).unwrap();
    assert!(
        sql.contains("CREATE TABLE IF NOT EXISTS cursors"),
        "schema must define cursors table"
    );
}

#[test]
fn schema_does_not_have_event_cache_table() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).unwrap();
    assert!(
        !sql.contains("event_cache"),
        "v1 schema must NOT contain event_cache table"
    );
}

#[test]
fn schema_does_not_have_stream_cursors_table() {
    let sql = std::fs::read_to_string(SCHEMA_PATH).unwrap();
    assert!(
        !sql.contains("stream_cursors"),
        "v1 schema must NOT contain stream_cursors table"
    );
}

// ---------------------------------------------------------------------------
// Integrity check
// ---------------------------------------------------------------------------

#[test]
fn integrity_check_passes_on_fresh_db() {
    let conn = open_memory_db();
    let result: String = conn
        .pragma_query_value(None, "integrity_check", |r| r.get(0))
        .unwrap();
    assert_eq!(result, "ok", "integrity_check must return 'ok' on fresh db");
}

// ---------------------------------------------------------------------------
// Profile table
// ---------------------------------------------------------------------------

#[test]
fn profile_insert_and_read() {
    let conn = open_memory_db();
    conn.execute(
        "INSERT INTO profile (server_url, token) VALUES (?1,?2)",
        rusqlite::params!["wss://example.com", "tok"],
    )
    .unwrap();
    let url: String = conn
        .query_row("SELECT server_url FROM profile", [], |r| r.get(0))
        .unwrap();
    assert_eq!(url, "wss://example.com");
}

#[test]
fn profile_write_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("r.db");
    {
        let c = open_file_db(&p);
        c.execute(
            "INSERT INTO profile (server_url,token) VALUES(?1,?2)",
            rusqlite::params!["wss://p.com", "t"],
        )
        .unwrap();
    }
    let c = reopen_file_db(&p);
    let url: String = c
        .query_row("SELECT server_url FROM profile", [], |r| r.get(0))
        .unwrap();
    assert_eq!(url, "wss://p.com");
}

// ---------------------------------------------------------------------------
// Subscriptions table
// ---------------------------------------------------------------------------

#[test]
fn subscriptions_insert_and_read() {
    let conn = open_memory_db();
    conn.execute(
        "INSERT INTO subscriptions (forwarder_id,reader_ip) VALUES(?1,?2)",
        rusqlite::params!["f", "192.168.1.100"],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO subscriptions (forwarder_id,reader_ip) VALUES(?1,?2)",
        rusqlite::params!["f", "192.168.1.200"],
    )
    .unwrap();
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM subscriptions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 2);
}

#[test]
fn subscriptions_pk_rejects_duplicate() {
    let conn = open_memory_db();
    conn.execute(
        "INSERT INTO subscriptions (forwarder_id,reader_ip) VALUES(?1,?2)",
        rusqlite::params!["f", "192.168.1.100"],
    )
    .unwrap();
    let result = conn.execute(
        "INSERT INTO subscriptions (forwarder_id,reader_ip) VALUES(?1,?2)",
        rusqlite::params!["f", "192.168.1.100"],
    );
    assert!(
        result.is_err(),
        "duplicate (forwarder_id, reader_ip) must be rejected"
    );
}

// ---------------------------------------------------------------------------
// Cursors table
// ---------------------------------------------------------------------------

#[test]
fn cursor_insert_and_read() {
    let conn = open_memory_db();
    conn.execute("INSERT INTO cursors (forwarder_id,reader_ip,stream_epoch,acked_through_seq) VALUES(?1,?2,?3,?4)", rusqlite::params!["f","i",3i64,17i64]).unwrap();
    let (e,s): (i64,i64) = conn.query_row("SELECT stream_epoch, acked_through_seq FROM cursors WHERE forwarder_id='f' AND reader_ip='i'", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
    assert_eq!(e, 3);
    assert_eq!(s, 17);
}

#[test]
fn cursor_pk_rejects_duplicate() {
    let conn = open_memory_db();
    conn.execute("INSERT INTO cursors (forwarder_id,reader_ip,stream_epoch,acked_through_seq) VALUES(?1,?2,?3,?4)", rusqlite::params!["f","i",1i64,10i64]).unwrap();
    let result = conn.execute("INSERT INTO cursors (forwarder_id,reader_ip,stream_epoch,acked_through_seq) VALUES(?1,?2,?3,?4)", rusqlite::params!["f","i",2i64,20i64]);
    assert!(
        result.is_err(),
        "duplicate (forwarder_id, reader_ip) in cursors must be rejected"
    );
}

#[test]
fn cursor_upsert_advances_position() {
    let conn = open_memory_db();
    conn.execute("INSERT INTO cursors (forwarder_id,reader_ip,stream_epoch,acked_through_seq) VALUES(?1,?2,?3,?4)", rusqlite::params!["f","i",1i64,5i64]).unwrap();
    conn.execute("INSERT OR REPLACE INTO cursors (forwarder_id,reader_ip,stream_epoch,acked_through_seq) VALUES(?1,?2,?3,?4)", rusqlite::params!["f","i",1i64,25i64]).unwrap();
    let s: i64 = conn
        .query_row(
            "SELECT acked_through_seq FROM cursors WHERE forwarder_id='f' AND reader_ip='i'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(s, 25);
}

#[test]
fn cursor_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("r.db");
    {
        let c = open_file_db(&p);
        c.execute("INSERT INTO cursors (forwarder_id,reader_ip,stream_epoch,acked_through_seq) VALUES(?1,?2,?3,?4)", rusqlite::params!["f","i",1i64,99i64]).unwrap();
    }
    let c = reopen_file_db(&p);
    let s: i64 = c
        .query_row(
            "SELECT acked_through_seq FROM cursors WHERE forwarder_id='f' AND reader_ip='i'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(s, 99, "cursor must survive close/reopen");
}
