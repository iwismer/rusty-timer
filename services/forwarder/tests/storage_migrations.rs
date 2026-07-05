use forwarder::storage::migrations::{SCHEMA_VERSION, integrity_check, migrate};
use rusqlite::Connection;

fn table_names(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .expect("prepare table query");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query tables")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect tables")
}

fn index_names(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT name FROM sqlite_schema WHERE type = 'index' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .expect("prepare index query");
    stmt.query_map([], |row| row.get::<_, String>(0))
        .expect("query indexes")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect indexes")
}

#[test]
fn migration_applies_on_empty_db() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("forwarder.db");
    let conn = Connection::open(&db_path).expect("open db");

    migrate(&conn).expect("migration applies");

    assert_eq!(
        table_names(&conn),
        vec![
            "events",
            "receiver_stream_cursors",
            "receivers",
            "stream_epochs",
            "stream_retention",
            "streams",
        ]
    );

    let wal_mode: String = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .expect("journal_mode");
    assert_eq!(wal_mode.to_lowercase(), "wal");

    let synchronous: i64 = conn
        .pragma_query_value(None, "synchronous", |row| row.get(0))
        .expect("synchronous");
    assert_eq!(synchronous, 2);

    let foreign_keys: i64 = conn
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .expect("foreign_keys");
    assert_eq!(foreign_keys, 1);
}

#[test]
fn integrity_check_passes() {
    let conn = Connection::open_in_memory().expect("open db");

    migrate(&conn).expect("migration applies");

    integrity_check(&conn).expect("integrity check passes");
}

#[test]
fn migrate_adds_retention_indexes_to_existing_v1_db() {
    let conn = Connection::open_in_memory().expect("open db");
    conn.execute_batch(
        "CREATE TABLE streams (
             stream_id TEXT PRIMARY KEY,
             hardware_reader_id TEXT NOT NULL,
             network_addr TEXT NOT NULL,
             display_name TEXT NOT NULL,
             reader_connected INTEGER NOT NULL DEFAULT 0,
             created_unix_ms INTEGER NOT NULL
         );
         CREATE TABLE stream_epochs (
             stream_id TEXT NOT NULL,
             epoch INTEGER NOT NULL,
             start_seq INTEGER NOT NULL,
             end_seq INTEGER,
             reason TEXT NOT NULL,
             PRIMARY KEY (stream_id, epoch),
             FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE
         );
         CREATE TABLE events (
             stream_id TEXT NOT NULL,
             seq INTEGER NOT NULL,
             epoch INTEGER NOT NULL,
             raw_frame BLOB NOT NULL,
             read_kind TEXT NOT NULL,
             reader_timestamp TEXT,
             received_unix_ms INTEGER NOT NULL,
             PRIMARY KEY (stream_id, seq),
             FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE,
             FOREIGN KEY (stream_id, epoch) REFERENCES stream_epochs(stream_id, epoch) ON DELETE CASCADE
         );
         CREATE TABLE receivers (
             endpoint_id TEXT PRIMARY KEY,
             display_name TEXT NOT NULL,
             approved_unix_ms INTEGER NOT NULL
         );
         CREATE TABLE receiver_stream_cursors (
             endpoint_id TEXT NOT NULL,
             stream_id TEXT NOT NULL,
             acked_through_seq INTEGER NOT NULL,
             PRIMARY KEY (endpoint_id, stream_id),
             FOREIGN KEY (endpoint_id) REFERENCES receivers(endpoint_id) ON DELETE CASCADE,
             FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE
         );
         CREATE TABLE stream_retention (
             stream_id TEXT PRIMARY KEY,
             earliest_available_seq INTEGER NOT NULL,
             forced_gap_count INTEGER NOT NULL,
             FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE
         );
         CREATE INDEX idx_events_stream_epoch_seq ON events(stream_id, epoch, seq);
         PRAGMA user_version = 1;",
    )
    .expect("create v1 schema without retention indexes");

    assert_eq!(
        index_names(&conn),
        vec!["idx_events_stream_epoch_seq"],
        "test setup must simulate a v1 database created before retention indexes existed"
    );

    migrate(&conn).expect("migration applies");

    assert_eq!(
        index_names(&conn),
        vec![
            "idx_cursors_stream_acked",
            "idx_events_received_stream_seq",
            "idx_events_stream_epoch_seq",
        ]
    );
}

#[test]
fn migrate_adds_epoch_name_to_v3_db() {
    let conn = Connection::open_in_memory().expect("open db");
    conn.execute_batch(
        "CREATE TABLE streams (
             stream_id TEXT PRIMARY KEY,
             hardware_reader_id TEXT NOT NULL,
             network_addr TEXT NOT NULL,
             display_name TEXT NOT NULL,
             reader_connected INTEGER NOT NULL DEFAULT 0,
             created_unix_ms INTEGER NOT NULL
         );
         CREATE TABLE stream_epochs (
             stream_id TEXT NOT NULL,
             epoch INTEGER NOT NULL,
             start_seq INTEGER NOT NULL,
             end_seq INTEGER,
             reason TEXT NOT NULL,
             created_unix_ms INTEGER,
             PRIMARY KEY (stream_id, epoch),
             FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE
         );
         CREATE TABLE events (
             stream_id TEXT NOT NULL,
             seq INTEGER NOT NULL,
             epoch INTEGER NOT NULL,
             raw_frame BLOB NOT NULL,
             read_kind TEXT NOT NULL,
             reader_timestamp TEXT,
             received_unix_ms INTEGER NOT NULL,
             PRIMARY KEY (stream_id, seq),
             FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE,
             FOREIGN KEY (stream_id, epoch) REFERENCES stream_epochs(stream_id, epoch) ON DELETE CASCADE
         );
         CREATE TABLE receivers (
             endpoint_id TEXT PRIMARY KEY,
             display_name TEXT NOT NULL,
             approved_unix_ms INTEGER NOT NULL
         );
         CREATE TABLE receiver_stream_cursors (
             endpoint_id TEXT NOT NULL,
             stream_id TEXT NOT NULL,
             acked_through_seq INTEGER NOT NULL,
             PRIMARY KEY (endpoint_id, stream_id),
             FOREIGN KEY (endpoint_id) REFERENCES receivers(endpoint_id) ON DELETE CASCADE,
             FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE
         );
         CREATE TABLE stream_retention (
             stream_id TEXT PRIMARY KEY,
             earliest_available_seq INTEGER NOT NULL,
             forced_gap_count INTEGER NOT NULL,
             FOREIGN KEY (stream_id) REFERENCES streams(stream_id) ON DELETE CASCADE
         );
         CREATE INDEX idx_events_stream_epoch_seq ON events(stream_id, epoch, seq);
         CREATE INDEX idx_events_received_stream_seq ON events(received_unix_ms, stream_id, seq);
         CREATE INDEX idx_cursors_stream_acked ON receiver_stream_cursors(stream_id, acked_through_seq);
         PRAGMA user_version = 3;",
    )
    .expect("create v3 schema without stream_epochs.name");

    migrate(&conn).expect("migration applies");

    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user_version");
    assert_eq!(version, i64::from(SCHEMA_VERSION));
    assert_eq!(version, 4);

    conn.execute_batch(
        "INSERT INTO streams (stream_id, hardware_reader_id, network_addr, display_name, reader_connected, created_unix_ms)
         VALUES ('stream-a', 'stream-a', 'stream-a', 'stream-a', 0, 1);
         INSERT INTO stream_epochs (stream_id, epoch, start_seq, end_seq, reason, created_unix_ms, name)
         VALUES ('stream-a', 1, 1, NULL, 'initial', 1, 'Race 1');",
    )
    .expect("insert epoch row with name");

    let name: Option<String> = conn
        .query_row(
            "SELECT name FROM stream_epochs WHERE stream_id = 'stream-a' AND epoch = 1",
            [],
            |row| row.get(0),
        )
        .expect("read epoch name");
    assert_eq!(name.as_deref(), Some("Race 1"));
}

#[test]
fn fresh_db_has_epoch_name_column() {
    let conn = Connection::open_in_memory().expect("open db");

    migrate(&conn).expect("migration applies");

    // A fresh database must match a migrated one: stream_epochs.name exists.
    let has_name: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('stream_epochs') WHERE name = 'name'",
            [],
            |row| row.get(0),
        )
        .expect("table info");
    assert_eq!(has_name, 1, "stream_epochs.name column missing");
}

#[test]
fn legacy_schema_is_rejected_loudly() {
    // A pre-P2P database: user_version stays 0 but legacy tables already exist.
    let conn = Connection::open_in_memory().expect("open db");
    conn.execute_batch(
        "CREATE TABLE journal (id INTEGER PRIMARY KEY);
         CREATE TABLE stream_state (stream_key TEXT PRIMARY KEY);",
    )
    .expect("create legacy tables");

    let err = migrate(&conn).expect_err("legacy schema must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("legacy"),
        "error must clearly flag the unsupported legacy schema, got: {msg}"
    );

    // The new schema must NOT have been overlaid on top of the legacy data.
    let tables = table_names(&conn);
    assert!(
        !tables.contains(&"events".to_owned()),
        "clean-slate schema must not be applied over legacy data, got: {tables:?}"
    );
}

#[test]
fn user_version_bumps() {
    let conn = Connection::open_in_memory().expect("open db");

    let before: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user_version before");
    assert_eq!(before, 0);

    migrate(&conn).expect("migration applies");

    let after: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("user_version after");
    assert_eq!(after, i64::from(SCHEMA_VERSION));
}
