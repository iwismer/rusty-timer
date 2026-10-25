use std::fmt::Write;

use rusqlite::{Connection, TransactionBehavior};

use super::sql_i64_to_u64;

const CHUNK_SIZE: usize = 64;

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

/// Clear all announcer rows and advance the announcer source generation in one
/// immediate transaction.
pub fn clear_announcer_rows(conn: &mut Connection) -> Result<u64, AnnouncerStorageError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute("DELETE FROM announcer_rows", [])?;
    tx.execute(
        "UPDATE announcer_source_state SET generation = generation + 1 WHERE id = 1",
        [],
    )?;
    let generation = current_announcer_source_generation(&tx)?;
    tx.commit()?;
    Ok(generation)
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
    conn: &mut Connection,
    rows: &[AnnouncerRowRecord],
) -> Result<(), AnnouncerStorageError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let current_generation = current_announcer_source_generation(&tx)?;
    if rows
        .iter()
        .any(|row| row.announcer_source_generation != current_generation)
    {
        return Err(AnnouncerStorageError::StaleGeneration { current_generation });
    }
    for chunk in rows.chunks(CHUNK_SIZE) {
        let mut sql = String::from(
            "INSERT INTO announcer_rows (
                 forwarder_endpoint_id, stream_id, seq, source_generation, chip_id, bib,
                 display_name, reader_timestamp, received_unix_ms, division
             )
             VALUES ",
        );
        for i in 0..chunk.len() {
            if i > 0 {
                sql.push_str(", ");
            }
            let base = i * 10;
            let _ = write!(
                sql,
                "(?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{}, ?{})",
                base + 1,
                base + 2,
                base + 3,
                base + 4,
                base + 5,
                base + 6,
                base + 7,
                base + 8,
                base + 9,
                base + 10
            );
        }
        sql.push_str(
            " ON CONFLICT(forwarder_endpoint_id, stream_id, seq) DO UPDATE SET
                 source_generation = excluded.source_generation,
                 chip_id = excluded.chip_id,
                 bib = excluded.bib,
                 display_name = excluded.display_name,
                 reader_timestamp = excluded.reader_timestamp,
                 received_unix_ms = excluded.received_unix_ms,
                 division = excluded.division",
        );

        let mut stmt = tx.prepare_cached(&sql)?;
        let mut converted = Vec::with_capacity(chunk.len());
        for row in chunk {
            let seq = u64_to_i64(row.seq, "seq")?;
            let generation = u64_to_i64(
                row.announcer_source_generation,
                "announcer_source_generation",
            )?;
            converted.push((seq, generation));
        }

        let mut params: Vec<&dyn rusqlite::types::ToSql> = Vec::with_capacity(chunk.len() * 10);
        for (row, (seq, generation)) in chunk.iter().zip(&converted) {
            params.push(&row.forwarder_endpoint_id);
            params.push(&row.stream_id);
            params.push(seq);
            params.push(generation);
            params.push(&row.chip_id);
            params.push(&row.bib);
            params.push(&row.display_name);
            params.push(&row.reader_timestamp);
            params.push(&row.received_unix_ms);
            params.push(&row.division);
        }
        stmt.execute(rusqlite::params_from_iter(params))?;
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
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::registry::migrate;

    static BUSY_SEEN: AtomicBool = AtomicBool::new(false);

    fn mark_busy_seen(_count: i32) -> bool {
        BUSY_SEEN.store(true, Ordering::SeqCst);
        true
    }

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
        let mut conn = test_conn();
        let err =
            upsert_announcer_rows(&mut conn, &[row("fwd-a", "finish-line", 1, 1)]).unwrap_err();
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
        let mut conn = test_conn();
        upsert_announcer_rows(
            &mut conn,
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
        let mut conn = test_conn();
        upsert_announcer_rows(&mut conn, &[row("fwd-a", "finish-line", 1, 0)]).unwrap();
        upsert_announcer_rows(&mut conn, &[row("fwd-a", "finish-line", 1, 0)]).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM announcer_rows", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1, "a replayed row is an idempotent no-op");
    }

    #[test]
    fn generation_fence_is_read_inside_immediate_write_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("server.sqlite3");
        let mut upsert_conn = crate::db::open(&db_path).unwrap();
        let blocker = crate::db::open(&db_path).unwrap();
        upsert_conn.busy_handler(Some(mark_busy_seen)).unwrap();
        BUSY_SEEN.store(false, Ordering::SeqCst);

        blocker.execute_batch("BEGIN IMMEDIATE;").unwrap();
        let upsert = std::thread::spawn(move || {
            upsert_announcer_rows(&mut upsert_conn, &[row("fwd-a", "finish-line", 1, 0)])
        });

        while !BUSY_SEEN.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        blocker
            .execute(
                "UPDATE announcer_source_state SET generation = generation + 1 WHERE id = 1",
                [],
            )
            .unwrap();
        blocker.execute_batch("COMMIT;").unwrap();

        let err = upsert.join().unwrap().unwrap_err();
        assert!(matches!(
            err,
            AnnouncerStorageError::StaleGeneration {
                current_generation: 1
            }
        ));

        let verifier = crate::db::open(&db_path).unwrap();
        let count: i64 = verifier
            .query_row("SELECT COUNT(*) FROM announcer_rows", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "stale rows must not slip in after a takeover");
    }

    #[test]
    fn clear_announcer_rows_deletes_rows_and_advances_generation() {
        let mut conn = test_conn();
        upsert_announcer_rows(
            &mut conn,
            &[
                row("fwd-a", "finish-line", 1, 0),
                row("fwd-b", "finish-line", 2, 0),
            ],
        )
        .unwrap();

        let initial_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM announcer_rows", [], |r| r.get(0))
            .unwrap();
        assert_eq!(initial_count, 2);
        assert_eq!(current_announcer_source_generation(&conn).unwrap(), 0);

        let next_gen = clear_announcer_rows(&mut conn).unwrap();
        assert_eq!(next_gen, 1);
        assert_eq!(current_announcer_source_generation(&conn).unwrap(), 1);

        let post_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM announcer_rows", [], |r| r.get(0))
            .unwrap();
        assert_eq!(post_count, 0, "all announcer rows must be deleted on clear");
    }
}
