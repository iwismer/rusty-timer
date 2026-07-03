use rusqlite::{Connection, params};

use super::sql_i64_to_u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnouncerRowRecord {
    pub announcer_source_generation: u64,
    /// Iroh endpoint id of the forwarder the row originated from. Storage and
    /// idempotency are keyed on `(forwarder_endpoint_id, stream_id, seq)`.
    pub forwarder_endpoint_id: String,
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

/// Persist a batch of announcer rows transactionally, fenced on the source
/// generation.
///
/// Every row's generation must exactly equal the current generation: any
/// mismatch (stale *or* future) rejects the WHOLE batch with
/// [`StaleGeneration`] and persists nothing. A source whose belief diverged
/// must call `/announcer/takeover` to re-fence before pushing again.
///
/// Replayed `(forwarder_endpoint_id, stream_id, seq)` rows upsert in place
/// (idempotent no-op for identical payloads) within an otherwise-accepted
/// batch. All-or-nothing: the transaction commits only if every row inserts.
///
/// [`StaleGeneration`]: AnnouncerStorageError::StaleGeneration
pub fn upsert_announcer_rows(
    conn: &Connection,
    rows: &[AnnouncerRowRecord],
) -> Result<(), AnnouncerStorageError> {
    let current_generation = current_announcer_source_generation(conn)?;
    if rows
        .iter()
        .any(|row| row.announcer_source_generation != current_generation)
    {
        return Err(AnnouncerStorageError::StaleGeneration { current_generation });
    }

    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO announcer_rows (
                 forwarder_endpoint_id, stream_id, seq, source_generation, chip_id, bib,
                 display_name, reader_timestamp, received_unix_ms, division
             )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(forwarder_endpoint_id, stream_id, seq) DO UPDATE SET
                 source_generation = excluded.source_generation,
                 chip_id = excluded.chip_id,
                 bib = excluded.bib,
                 display_name = excluded.display_name,
                 reader_timestamp = excluded.reader_timestamp,
                 received_unix_ms = excluded.received_unix_ms,
                 division = excluded.division",
        )?;
        for row in rows {
            stmt.execute(params![
                &row.forwarder_endpoint_id,
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
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn list_announcer_rows_ordered(
    conn: &Connection,
) -> Result<Vec<AnnouncerRowRecord>, AnnouncerStorageError> {
    let mut stmt = conn.prepare(
        "SELECT source_generation, forwarder_endpoint_id, stream_id, seq, chip_id, bib,
                display_name, reader_timestamp, received_unix_ms, division
         FROM announcer_rows
         ORDER BY received_unix_ms, forwarder_endpoint_id, stream_id, seq",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(AnnouncerRowRecord {
            announcer_source_generation: sql_i64_to_u64(row.get(0)?, 0)?,
            forwarder_endpoint_id: row.get(1)?,
            stream_id: row.get(2)?,
            seq: sql_i64_to_u64(row.get(3)?, 3)?,
            chip_id: row.get(4)?,
            bib: row.get(5)?,
            display_name: row.get(6)?,
            reader_timestamp: row.get(7)?,
            received_unix_ms: row.get(8)?,
            division: row.get(9)?,
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

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        conn
    }

    fn row(
        forwarder_endpoint_id: &str,
        stream_id: &str,
        seq: u64,
        generation: u64,
    ) -> AnnouncerRowRecord {
        AnnouncerRowRecord {
            announcer_source_generation: generation,
            forwarder_endpoint_id: forwarder_endpoint_id.to_string(),
            stream_id: stream_id.to_string(),
            seq,
            chip_id: format!("chip-{seq}"),
            bib: Some(1001),
            display_name: format!("Runner {seq}"),
            reader_timestamp: None,
            received_unix_ms: i64::try_from(seq).unwrap() * 1_000,
            division: None,
        }
    }

    #[test]
    fn upsert_announcer_rows_rejects_generation_above_current() {
        let conn = test_conn();
        let err = upsert_announcer_rows(&conn, &[row("fwd-a", "finish-line", 1, 1)]).unwrap_err();
        assert!(matches!(
            err,
            AnnouncerStorageError::StaleGeneration {
                current_generation: 0
            }
        ));
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM announcer_rows", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "a fenced-out batch must persist nothing");
    }

    #[test]
    fn same_wire_stream_id_from_different_forwarders_does_not_collide() {
        let conn = test_conn();
        upsert_announcer_rows(
            &conn,
            &[
                row("fwd-a", "finish-line", 1, 0),
                row("fwd-b", "finish-line", 1, 0),
            ],
        )
        .unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM announcer_rows WHERE stream_id = 'finish-line' AND seq = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            count, 2,
            "two forwarders with the same wire stream id must store distinct rows"
        );
    }

    #[test]
    fn replayed_composite_key_upserts_in_place() {
        let conn = test_conn();
        upsert_announcer_rows(&conn, &[row("fwd-a", "finish-line", 1, 0)]).unwrap();
        upsert_announcer_rows(&conn, &[row("fwd-a", "finish-line", 1, 0)]).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM announcer_rows", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "a replayed row is an idempotent no-op");
    }
}
