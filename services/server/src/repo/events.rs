use std::convert::TryFrom;

use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, PartialEq, Eq)]
pub enum IngestResult {
    Inserted,
    Retransmit,
    IntegrityConflict,
}

#[derive(Debug, PartialEq, Eq)]
pub struct UpsertEventOutcome {
    pub ingest_result: IngestResult,
    pub epoch_advanced_to: Option<i64>,
}

pub async fn upsert_event(
    pool: &PgPool,
    stream_id: Uuid,
    stream_epoch: i64,
    seq: i64,
    reader_timestamp: &str,
    raw_read_line: &str,
    read_type: &str,
) -> Result<UpsertEventOutcome, sqlx::Error> {
    let tag_id = ipico_core::read::ChipRead::try_from(raw_read_line)
        .ok()
        .map(|r| r.tag_id);
    let mut tx = pool.begin().await?;
    let mut current_stream_epoch = sqlx::query_scalar::<_, i64>(
        "SELECT stream_epoch FROM streams WHERE stream_id = $1 FOR UPDATE",
    )
    .bind(stream_id)
    .fetch_optional(&mut *tx)
    .await?
    .unwrap_or(stream_epoch);
    let mut epoch_advanced_to = None;

    if stream_epoch > current_stream_epoch {
        sqlx::query("UPDATE streams SET stream_epoch = $2 WHERE stream_id = $1")
            .bind(stream_id)
            .bind(stream_epoch)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE stream_metrics SET epoch_raw_count = 0, epoch_dedup_count = 0, epoch_retransmit_count = 0, epoch_last_received_at = NULL, last_tag_id = NULL, last_reader_timestamp = NULL WHERE stream_id = $1",
        )
        .bind(stream_id)
        .execute(&mut *tx)
        .await?;
        current_stream_epoch = stream_epoch;
        epoch_advanced_to = Some(stream_epoch);
    }

    let existing_raw_read_line = sqlx::query_scalar::<_, String>(
        "SELECT raw_read_line FROM events WHERE stream_id = $1 AND stream_epoch = $2 AND seq = $3",
    )
    .bind(stream_id)
    .bind(stream_epoch)
    .bind(seq)
    .fetch_optional(&mut *tx)
    .await?;
    let is_current_epoch = stream_epoch == current_stream_epoch;

    let ingest_result = if let Some(existing_raw_read_line) = existing_raw_read_line {
        if existing_raw_read_line == raw_read_line {
            if is_current_epoch {
                sqlx::query(
                    "UPDATE stream_metrics SET raw_count = raw_count + 1, retransmit_count = retransmit_count + 1, epoch_raw_count = epoch_raw_count + 1, epoch_retransmit_count = epoch_retransmit_count + 1 WHERE stream_id = $1",
                )
                .bind(stream_id)
                .execute(&mut *tx)
                .await?;
            } else {
                sqlx::query(
                    "UPDATE stream_metrics SET raw_count = raw_count + 1, retransmit_count = retransmit_count + 1 WHERE stream_id = $1",
                )
                .bind(stream_id)
                .execute(&mut *tx)
                .await?;
            }
            IngestResult::Retransmit
        } else {
            IngestResult::IntegrityConflict
        }
    } else {
        sqlx::query(
            r#"INSERT INTO events (stream_id, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, tag_id) VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
        )
        .bind(stream_id)
        .bind(stream_epoch)
        .bind(seq)
        .bind(reader_timestamp)
        .bind(raw_read_line)
        .bind(read_type)
        .bind(tag_id.as_deref())
        .execute(&mut *tx)
        .await?;
        if is_current_epoch {
            sqlx::query(
                "UPDATE stream_metrics SET raw_count = raw_count + 1, dedup_count = dedup_count + 1, last_canonical_event_received_at = now(), epoch_raw_count = epoch_raw_count + 1, epoch_dedup_count = epoch_dedup_count + 1, epoch_last_received_at = now(), last_tag_id = $2, last_reader_timestamp = $3 WHERE stream_id = $1",
            )
            .bind(stream_id)
            .bind(tag_id.as_deref())
            .bind(reader_timestamp)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query(
                "UPDATE stream_metrics SET raw_count = raw_count + 1, dedup_count = dedup_count + 1, last_canonical_event_received_at = now() WHERE stream_id = $1",
            )
            .bind(stream_id)
            .execute(&mut *tx)
            .await?;
        }
        IngestResult::Inserted
    };

    tx.commit().await?;
    Ok(UpsertEventOutcome {
        ingest_result,
        epoch_advanced_to,
    })
}

pub async fn upsert_stream(
    pool: &PgPool,
    forwarder_id: &str,
    reader_ip: &str,
    forwarder_display_name: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    let row = sqlx::query!(
        r#"INSERT INTO streams (forwarder_id, reader_ip, forwarder_display_name) VALUES ($1, $2, $3)
           ON CONFLICT (forwarder_id, reader_ip) DO UPDATE SET forwarder_display_name = EXCLUDED.forwarder_display_name
           RETURNING stream_id"#,
        forwarder_id,
        reader_ip,
        forwarder_display_name
    )
    .fetch_one(pool)
    .await?;
    let stream_id = row.stream_id;
    sqlx::query!(
        "INSERT INTO stream_metrics (stream_id) VALUES ($1) ON CONFLICT (stream_id) DO NOTHING",
        stream_id
    )
    .execute(pool)
    .await?;
    Ok(stream_id)
}

pub async fn update_forwarder_display_name(
    pool: &PgPool,
    forwarder_id: &str,
    forwarder_display_name: Option<&str>,
) -> Result<u64, sqlx::Error> {
    let result =
        sqlx::query("UPDATE streams SET forwarder_display_name = $1 WHERE forwarder_id = $2")
            .bind(forwarder_display_name)
            .bind(forwarder_id)
            .execute(pool)
            .await?;
    Ok(result.rows_affected())
}

pub async fn fetch_stream_ids_by_forwarder(
    pool: &PgPool,
    forwarder_id: &str,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let rows =
        sqlx::query_scalar::<_, Uuid>("SELECT stream_id FROM streams WHERE forwarder_id = $1")
            .bind(forwarder_id)
            .fetch_all(pool)
            .await?;
    Ok(rows)
}

pub async fn set_stream_online(
    pool: &PgPool,
    stream_id: Uuid,
    online: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE streams SET online = $1 WHERE stream_id = $2",
        online,
        stream_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

pub struct StreamMetricsRow {
    pub raw_count: i64,
    pub dedup_count: i64,
    pub retransmit_count: i64,
    pub lag_ms: Option<u64>,
    pub epoch_raw_count: i64,
    pub epoch_dedup_count: i64,
    pub epoch_retransmit_count: i64,
    pub epoch_lag_ms: Option<u64>,
    pub epoch_last_received_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_tag_id: Option<String>,
    pub last_reader_timestamp: Option<String>,
}

pub struct StreamSnapshotRow {
    pub stream_id: Uuid,
    pub forwarder_id: String,
    pub reader_ip: String,
    pub display_alias: Option<String>,
    pub forwarder_display_name: Option<String>,
    pub online: bool,
    pub stream_epoch: i64,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn fetch_stream_metrics(
    pool: &PgPool,
    stream_id: Uuid,
) -> Result<Option<StreamMetricsRow>, sqlx::Error> {
    let row = sqlx::query(
        r#"SELECT raw_count, dedup_count, retransmit_count, last_canonical_event_received_at,
                  epoch_raw_count, epoch_dedup_count, epoch_retransmit_count, epoch_last_received_at,
                  last_tag_id, last_reader_timestamp
           FROM stream_metrics WHERE stream_id = $1"#,
    )
    .bind(stream_id)
    .fetch_optional(pool)
    .await?;

    let now = chrono::Utc::now();
    Ok(row.map(|r| {
        let last_canonical: Option<chrono::DateTime<chrono::Utc>> =
            r.get("last_canonical_event_received_at");
        let epoch_last: Option<chrono::DateTime<chrono::Utc>> = r.get("epoch_last_received_at");
        let lag_ms = last_canonical.map(|ts| (now - ts).num_milliseconds().max(0) as u64);
        let epoch_lag_ms = epoch_last.map(|ts| (now - ts).num_milliseconds().max(0) as u64);
        StreamMetricsRow {
            raw_count: r.get("raw_count"),
            dedup_count: r.get("dedup_count"),
            retransmit_count: r.get("retransmit_count"),
            lag_ms,
            epoch_raw_count: r.get("epoch_raw_count"),
            epoch_dedup_count: r.get("epoch_dedup_count"),
            epoch_retransmit_count: r.get("epoch_retransmit_count"),
            epoch_lag_ms,
            epoch_last_received_at: epoch_last,
            last_tag_id: r.get("last_tag_id"),
            last_reader_timestamp: r.get("last_reader_timestamp"),
        }
    }))
}

pub async fn count_unique_chips(
    pool: &PgPool,
    stream_id: Uuid,
    stream_epoch: i64,
) -> Result<i64, sqlx::Error> {
    let count = sqlx::query_scalar!(
        "SELECT COUNT(DISTINCT tag_id) FROM events WHERE stream_id = $1 AND stream_epoch = $2 AND tag_id IS NOT NULL",
        stream_id,
        stream_epoch
    )
    .fetch_one(pool)
    .await?;
    Ok(count.unwrap_or(0))
}

pub async fn fetch_stream_snapshot(
    pool: &PgPool,
    stream_id: Uuid,
) -> Result<Option<StreamSnapshotRow>, sqlx::Error> {
    let row = sqlx::query(
        r#"SELECT stream_id, forwarder_id, reader_ip, display_alias, forwarder_display_name, online, stream_epoch, created_at
           FROM streams WHERE stream_id = $1"#,
    )
    .bind(stream_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| StreamSnapshotRow {
        stream_id: r.get("stream_id"),
        forwarder_id: r.get("forwarder_id"),
        reader_ip: r.get("reader_ip"),
        display_alias: r.get("display_alias"),
        forwarder_display_name: r.get("forwarder_display_name"),
        online: r.get("online"),
        stream_epoch: r.get("stream_epoch"),
        created_at: r.get("created_at"),
    }))
}

pub async fn fetch_events_after_cursor(
    pool: &PgPool,
    stream_id: Uuid,
    after_epoch: i64,
    after_seq: i64,
) -> Result<Vec<crate::repo::EventRow>, sqlx::Error> {
    fetch_events_after_cursor_limited(pool, stream_id, after_epoch, after_seq, i64::MAX).await
}

pub async fn fetch_events_after_cursor_limited(
    pool: &PgPool,
    stream_id: Uuid,
    after_epoch: i64,
    after_seq: i64,
    limit: i64,
) -> Result<Vec<crate::repo::EventRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT e.stream_epoch, e.seq, e.reader_timestamp, e.raw_read_line, e.read_type,
                  s.forwarder_id, s.reader_ip
           FROM events e
           JOIN streams s ON s.stream_id = e.stream_id
           WHERE e.stream_id = $1 AND (e.stream_epoch > $2 OR (e.stream_epoch = $2 AND e.seq > $3))
           ORDER BY e.stream_epoch ASC, e.seq ASC
           LIMIT $4"#,
    )
    .bind(stream_id)
    .bind(after_epoch)
    .bind(after_seq)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| crate::repo::EventRow {
            stream_epoch: r.get("stream_epoch"),
            seq: r.get("seq"),
            reader_timestamp: r.get("reader_timestamp"),
            raw_read_line: r.get("raw_read_line"),
            read_type: r.get("read_type"),
            forwarder_id: r.get("forwarder_id"),
            reader_ip: r.get("reader_ip"),
        })
        .collect())
}

pub async fn fetch_max_event_cursor(
    pool: &PgPool,
    stream_id: Uuid,
) -> Result<Option<(i64, i64)>, sqlx::Error> {
    let row = sqlx::query(
        r#"SELECT stream_epoch, seq
           FROM events
           WHERE stream_id = $1
           ORDER BY stream_epoch DESC, seq DESC
           LIMIT 1"#,
    )
    .bind(stream_id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| (r.get("stream_epoch"), r.get("seq"))))
}

pub async fn fetch_events_after_cursor_through_cursor_limited(
    pool: &PgPool,
    stream_id: Uuid,
    after_epoch: i64,
    after_seq: i64,
    through_epoch: i64,
    through_seq: i64,
    limit: i64,
) -> Result<Vec<crate::repo::EventRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT e.stream_epoch, e.seq, e.reader_timestamp, e.raw_read_line, e.read_type,
                  s.forwarder_id, s.reader_ip
           FROM events e
           JOIN streams s ON s.stream_id = e.stream_id
           WHERE e.stream_id = $1
             AND (e.stream_epoch > $2 OR (e.stream_epoch = $2 AND e.seq > $3))
             AND (e.stream_epoch < $4 OR (e.stream_epoch = $4 AND e.seq <= $5))
           ORDER BY e.stream_epoch ASC, e.seq ASC
           LIMIT $6"#,
    )
    .bind(stream_id)
    .bind(after_epoch)
    .bind(after_seq)
    .bind(through_epoch)
    .bind(through_seq)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| crate::repo::EventRow {
            stream_epoch: r.get("stream_epoch"),
            seq: r.get("seq"),
            reader_timestamp: r.get("reader_timestamp"),
            raw_read_line: r.get("raw_read_line"),
            read_type: r.get("read_type"),
            forwarder_id: r.get("forwarder_id"),
            reader_ip: r.get("reader_ip"),
        })
        .collect())
}

pub async fn fetch_events_for_stream_epoch_from_seq_limited(
    pool: &PgPool,
    stream_id: Uuid,
    stream_epoch: i64,
    from_seq: i64,
    limit: i64,
) -> Result<Vec<crate::repo::EventRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT e.stream_epoch, e.seq, e.reader_timestamp, e.raw_read_line, e.read_type,
                  s.forwarder_id, s.reader_ip
           FROM events e
           JOIN streams s ON s.stream_id = e.stream_id
           WHERE e.stream_id = $1
             AND e.stream_epoch = $2
             AND e.seq >= $3
           ORDER BY e.seq ASC
           LIMIT $4"#,
    )
    .bind(stream_id)
    .bind(stream_epoch)
    .bind(from_seq)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| crate::repo::EventRow {
            stream_epoch: r.get("stream_epoch"),
            seq: r.get("seq"),
            reader_timestamp: r.get("reader_timestamp"),
            raw_read_line: r.get("raw_read_line"),
            read_type: r.get("read_type"),
            forwarder_id: r.get("forwarder_id"),
            reader_ip: r.get("reader_ip"),
        })
        .collect())
}

pub async fn fetch_events_for_stream_epoch_from_seq_through_cursor_limited(
    pool: &PgPool,
    stream_id: Uuid,
    stream_epoch: i64,
    from_seq: i64,
    through_epoch: i64,
    through_seq: i64,
    limit: i64,
) -> Result<Vec<crate::repo::EventRow>, sqlx::Error> {
    let rows = sqlx::query(
        r#"SELECT e.stream_epoch, e.seq, e.reader_timestamp, e.raw_read_line, e.read_type,
                  s.forwarder_id, s.reader_ip
           FROM events e
           JOIN streams s ON s.stream_id = e.stream_id
           WHERE e.stream_id = $1
             AND e.stream_epoch = $2
             AND e.seq >= $3
             AND (e.stream_epoch < $4 OR (e.stream_epoch = $4 AND e.seq <= $5))
           ORDER BY e.seq ASC
           LIMIT $6"#,
    )
    .bind(stream_id)
    .bind(stream_epoch)
    .bind(from_seq)
    .bind(through_epoch)
    .bind(through_seq)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| crate::repo::EventRow {
            stream_epoch: r.get("stream_epoch"),
            seq: r.get("seq"),
            reader_timestamp: r.get("reader_timestamp"),
            raw_read_line: r.get("raw_read_line"),
            read_type: r.get("read_type"),
            forwarder_id: r.get("forwarder_id"),
            reader_ip: r.get("reader_ip"),
        })
        .collect())
}
