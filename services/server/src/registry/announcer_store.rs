use rusqlite::{Connection, params};

use super::sql_i64_to_u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncerRowRecord {
    pub announcer_source_generation: u64,
    pub stream_id: String,
    pub seq: u64,
    pub chip_id: String,
    pub bib: Option<i32>,
    pub display_name: String,
    pub reader_timestamp: Option<String>,
    pub received_unix_ms: i64,
    /// Division display name resolved by the receiver, when known.
    pub division: Option<String>,
}

#[derive(Debug)]
pub enum AnnouncerStorageError {
    Sqlite(rusqlite::Error),
    StaleGeneration { current_generation: u64 },
    ValueOutOfRange(&'static str),
}

impl From<rusqlite::Error> for AnnouncerStorageError {
    fn from(err: rusqlite::Error) -> Self {
        Self::Sqlite(err)
    }
}

pub fn current_announcer_source_generation(
    conn: &Connection,
) -> Result<u64, AnnouncerStorageError> {
    let generation = conn.query_row(
        "SELECT generation FROM announcer_source_state WHERE id = 1",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    u64::try_from(generation).map_err(|_| AnnouncerStorageError::ValueOutOfRange("generation"))
}

pub fn takeover_announcer_source(conn: &Connection) -> Result<u64, AnnouncerStorageError> {
    conn.execute(
        "UPDATE announcer_source_state SET generation = generation + 1 WHERE id = 1",
        [],
    )?;
    current_announcer_source_generation(conn)
}

/// Persist an announcer row, fenced on the source generation.
///
/// The row's generation must exactly equal the current generation: any
/// mismatch (stale *or* future) is rejected with [`StaleGeneration`]. A source
/// whose belief diverged must call `/announcer/takeover` to re-fence before
/// pushing again.
///
/// [`StaleGeneration`]: AnnouncerStorageError::StaleGeneration
pub fn upsert_announcer_row(
    conn: &Connection,
    row: &AnnouncerRowRecord,
) -> Result<(), AnnouncerStorageError> {
    let current_generation = current_announcer_source_generation(conn)?;
    if row.announcer_source_generation != current_generation {
        return Err(AnnouncerStorageError::StaleGeneration { current_generation });
    }

    conn.execute(
        "INSERT INTO announcer_rows (
             stream_id, seq, source_generation, chip_id, bib, display_name,
             reader_timestamp, received_unix_ms, division
         )
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(stream_id, seq) DO UPDATE SET
             source_generation = excluded.source_generation,
             chip_id = excluded.chip_id,
             bib = excluded.bib,
             display_name = excluded.display_name,
             reader_timestamp = excluded.reader_timestamp,
             received_unix_ms = excluded.received_unix_ms,
             division = excluded.division",
        params![
            &row.stream_id,
            u64_to_i64(row.seq, "seq")?,
            u64_to_i64(
                row.announcer_source_generation,
                "announcer_source_generation"
            )?,
            &row.chip_id,
            row.bib,
            &row.display_name,
            &row.reader_timestamp,
            row.received_unix_ms,
            &row.division,
        ],
    )?;
    Ok(())
}

pub fn list_announcer_rows_ordered(
    conn: &Connection,
) -> Result<Vec<AnnouncerRowRecord>, AnnouncerStorageError> {
    let mut stmt = conn.prepare(
        "SELECT source_generation, stream_id, seq, chip_id, bib, display_name,
                reader_timestamp, received_unix_ms, division
         FROM announcer_rows
         ORDER BY received_unix_ms, stream_id, seq",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AnnouncerRowRecord {
            announcer_source_generation: sql_i64_to_u64(row.get(0)?, 0)?,
            stream_id: row.get(1)?,
            seq: sql_i64_to_u64(row.get(2)?, 2)?,
            chip_id: row.get(3)?,
            bib: row.get(4)?,
            display_name: row.get(5)?,
            reader_timestamp: row.get(6)?,
            received_unix_ms: row.get(7)?,
            division: row.get(8)?,
        })
    })?;

    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AnnouncerStorageError::Sqlite)
}

fn u64_to_i64(value: u64, field: &'static str) -> Result<i64, AnnouncerStorageError> {
    i64::try_from(value).map_err(|_| AnnouncerStorageError::ValueOutOfRange(field))
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;
    use crate::registry::migrate;

    #[test]
    fn upsert_announcer_row_rejects_generation_above_current() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        migrate(&conn).unwrap();

        let row = AnnouncerRowRecord {
            announcer_source_generation: 1,
            stream_id: "finish-line".to_string(),
            seq: 1,
            chip_id: "chip-1".to_string(),
            bib: Some(1001),
            display_name: "Runner 1".to_string(),
            reader_timestamp: None,
            received_unix_ms: 1_000,
            division: None,
        };

        let err = upsert_announcer_row(&conn, &row).unwrap_err();
        assert!(matches!(
            err,
            AnnouncerStorageError::StaleGeneration {
                current_generation: 0
            }
        ));
    }
}
