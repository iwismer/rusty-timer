use crate::storage::journal::JournalError;
use rusqlite::Connection;

pub const SCHEMA_VERSION: u32 = 1;

pub fn migrate(conn: &Connection) -> Result<(), JournalError> {
    apply_pragmas(conn)?;

    let current_version = user_version(conn)?;
    match current_version {
        0 => {
            if has_legacy_tables(conn)? {
                return Err(JournalError::InvalidData(
                    "unsupported legacy storage schema detected (found 'journal'/'stream_state' \
                     tables with user_version=0); the clean-slate P2P schema cannot migrate or \
                     import legacy data"
                        .to_owned(),
                ));
            }
            conn.execute_batch(&format!(
                "BEGIN IMMEDIATE;\n{}\nPRAGMA user_version = {};\nCOMMIT;",
                include_str!("schema.sql"),
                SCHEMA_VERSION
            ))?;
        }
        version if version == SCHEMA_VERSION => {}
        version => {
            return Err(JournalError::InvalidData(format!(
                "unsupported storage schema version {version}"
            )));
        }
    }

    integrity_check(conn)
}

pub fn integrity_check(conn: &Connection) -> Result<(), JournalError> {
    let result: String = conn.pragma_query_value(None, "integrity_check", |row| row.get(0))?;
    if result != "ok" {
        return Err(JournalError::IntegrityCheckFailed(result));
    }
    Ok(())
}

fn user_version(conn: &Connection) -> Result<u32, JournalError> {
    conn.pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(Into::into)
}

/// Detect tables from the pre-P2P (legacy) forwarder schema.
///
/// A legacy database carries `user_version = 0` but already contains the old
/// `journal`/`stream_state` tables. We must not treat it as an empty database
/// and silently overlay the new schema, which would hide the legacy data.
fn has_legacy_tables(conn: &Connection) -> Result<bool, JournalError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name IN ('journal', 'stream_state')",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn apply_pragmas(conn: &Connection) -> Result<(), JournalError> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=FULL;
         PRAGMA wal_autocheckpoint=1000;
         PRAGMA foreign_keys=ON;",
    )?;
    Ok(())
}
