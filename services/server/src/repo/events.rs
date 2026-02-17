use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, PartialEq, Eq)]
pub enum IngestResult { Inserted, Retransmit, IntegrityConflict }

pub async fn upsert_event(
    pool: &PgPool, stream_id: Uuid, stream_epoch: i64, seq: i64,
    reader_timestamp: &str, raw_read_line: &str, read_type: &str,
) -> Result<IngestResult, sqlx::Error> {
    let existing = sqlx::query!(
        "SELECT raw_read_line FROM events WHERE stream_id = $1 AND stream_epoch = $2 AND seq = $3",
        stream_id, stream_epoch, seq
    ).fetch_optional(pool).await?;

    if let Some(existing_row) = existing {
        if existing_row.raw_read_line == raw_read_line {
            sqlx::query!(
                "UPDATE stream_metrics SET raw_count = raw_count + 1, retransmit_count = retransmit_count + 1 WHERE stream_id = $1",
                stream_id
            ).execute(pool).await?;
            Ok(IngestResult::Retransmit)
        } else {
            Ok(IngestResult::IntegrityConflict)
        }
    } else {
        sqlx::query!(
            r#"INSERT INTO events (stream_id, stream_epoch, seq, reader_timestamp, raw_read_line, read_type) VALUES ($1, $2, $3, $4, $5, $6)"#,
            stream_id, stream_epoch, seq, reader_timestamp, raw_read_line, read_type
        ).execute(pool).await?;
        sqlx::query!(
            "UPDATE stream_metrics SET raw_count = raw_count + 1, dedup_count = dedup_count + 1, last_canonical_event_received_at = now() WHERE stream_id = $1",
            stream_id
        ).execute(pool).await?;
        Ok(IngestResult::Inserted)
    }
}

pub async fn upsert_stream(pool: &PgPool, forwarder_id: &str, reader_ip: &str) -> Result<Uuid, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO streams (forwarder_id, reader_ip) VALUES ($1, $2)
           ON CONFLICT (forwarder_id, reader_ip) DO UPDATE SET forwarder_id = EXCLUDED.forwarder_id
           RETURNING stream_id"#,
        forwarder_id, reader_ip
    ).fetch_one(pool).await?;
    let stream_id = row.stream_id;
    sqlx::query!(
        "INSERT INTO stream_metrics (stream_id) VALUES ($1) ON CONFLICT (stream_id) DO NOTHING",
        stream_id
    ).execute(pool).await?;
    Ok(stream_id)
}

pub async fn set_stream_online(pool: &PgPool, stream_id: Uuid, online: bool) -> Result<(), sqlx::Error> {
    sqlx::query!("UPDATE streams SET online = $1 WHERE stream_id = $2", online, stream_id)
        .execute(pool).await?;
    Ok(())
}

pub async fn fetch_events_after_cursor(
    pool: &PgPool, stream_id: Uuid, after_epoch: i64, after_seq: i64,
) -> Result<Vec<crate::repo::EventRow>, sqlx::Error> {
    let rows = sqlx::query_as!(
        crate::repo::EventRow,
        r#"SELECT e.stream_epoch, e.seq, e.reader_timestamp, e.raw_read_line, e.read_type,
                  s.forwarder_id, s.reader_ip
           FROM events e
           JOIN streams s ON s.stream_id = e.stream_id
           WHERE e.stream_id = $1 AND (e.stream_epoch > $2 OR (e.stream_epoch = $2 AND e.seq > $3))
           ORDER BY e.stream_epoch ASC, e.seq ASC"#,
        stream_id, after_epoch, after_seq
    ).fetch_all(pool).await?;
    Ok(rows)
}
