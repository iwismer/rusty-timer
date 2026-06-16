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
