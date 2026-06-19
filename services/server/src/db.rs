use std::path::Path;

use rusqlite::Connection;

const CURRENT_USER_VERSION: i64 = 2;

pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    migrate(&conn)?;
    crate::registry::migrate(&conn)?;
    Ok(conn)
}

pub fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE IF NOT EXISTS announcer_rows (
             stream_id TEXT NOT NULL,
             seq INTEGER NOT NULL,
             source_generation INTEGER NOT NULL DEFAULT 0,
             chip_id TEXT NOT NULL,
             bib INTEGER,
             display_name TEXT NOT NULL,
             reader_timestamp TEXT,
             received_unix_ms INTEGER NOT NULL,
             PRIMARY KEY(stream_id, seq)
         );
         CREATE TABLE IF NOT EXISTS announcer_source_state (
             id INTEGER PRIMARY KEY CHECK (id = 1),
             generation INTEGER NOT NULL
         );
         INSERT OR IGNORE INTO announcer_source_state (id, generation) VALUES (1, 0);",
    )?;

    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if user_version < 2 && !has_column(conn, "announcer_rows", "source_generation")? {
        conn.execute(
            "ALTER TABLE announcer_rows
             ADD COLUMN source_generation INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if user_version < CURRENT_USER_VERSION {
        conn.pragma_update(None, "user_version", CURRENT_USER_VERSION)?;
    }

    Ok(())
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
