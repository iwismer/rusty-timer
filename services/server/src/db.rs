use std::path::Path;

use rusqlite::Connection;

const CURRENT_USER_VERSION: i64 = 4;

pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    migrate(&conn)?;
    crate::registry::migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    // v4: announcer rows are keyed by the composite stream identity
    // (forwarder_endpoint_id, stream_id, seq) so two forwarders exposing the
    // same wire stream id never collide. Breaking rebuild of a pre-v4 table:
    // the system is undeployed, so the legacy single-stream-id rows are
    // dropped rather than migrated.
    if user_version < 4
        && has_table(conn, "announcer_rows")?
        && !has_column(conn, "announcer_rows", "forwarder_endpoint_id")?
    {
        conn.execute("DROP TABLE announcer_rows", [])?;
    }

    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS announcer_rows (
             forwarder_endpoint_id TEXT NOT NULL,
             stream_id TEXT NOT NULL,
             seq INTEGER NOT NULL,
             source_generation INTEGER NOT NULL DEFAULT 0,
             chip_id TEXT NOT NULL,
             bib INTEGER,
             display_name TEXT NOT NULL,
             reader_timestamp TEXT,
             received_unix_ms INTEGER NOT NULL,
             division TEXT,
             PRIMARY KEY(forwarder_endpoint_id, stream_id, seq)
         );
         CREATE TABLE IF NOT EXISTS announcer_source_state (
             id INTEGER PRIMARY KEY CHECK (id = 1),
             generation INTEGER NOT NULL
         );
         INSERT OR IGNORE INTO announcer_source_state (id, generation) VALUES (1, 0);",
    )?;

    if user_version < CURRENT_USER_VERSION {
        conn.pragma_update(None, "user_version", CURRENT_USER_VERSION)?;
    }

    Ok(())
}

fn has_table(conn: &Connection, table: &str) -> rusqlite::Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}
