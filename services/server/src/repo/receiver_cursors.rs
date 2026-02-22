use sqlx::PgPool;
use uuid::Uuid;

pub async fn upsert_cursor(
    pool: &PgPool,
    receiver_id: &str,
    stream_id: Uuid,
    stream_epoch: i64,
    last_seq: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"INSERT INTO receiver_cursors (receiver_id, stream_id, stream_epoch, last_seq, updated_at)
           VALUES ($1, $2, $3, $4, now())
           ON CONFLICT (receiver_id, stream_id) DO UPDATE
           SET stream_epoch = EXCLUDED.stream_epoch,
               last_seq = EXCLUDED.last_seq,
               updated_at = now()
           WHERE EXCLUDED.stream_epoch > receiver_cursors.stream_epoch
              OR (EXCLUDED.stream_epoch = receiver_cursors.stream_epoch
                  AND EXCLUDED.last_seq >= receiver_cursors.last_seq)"#,
    )
    .bind(receiver_id)
    .bind(stream_id)
    .bind(stream_epoch)
    .bind(last_seq)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn fetch_cursor(
    pool: &PgPool,
    receiver_id: &str,
    stream_id: Uuid,
) -> Result<Option<(i64, i64)>, sqlx::Error> {
    let row = sqlx::query!(
        "SELECT stream_epoch, last_seq FROM receiver_cursors WHERE receiver_id = $1 AND stream_id = $2",
        receiver_id, stream_id
    ).fetch_optional(pool).await?;
    Ok(row.map(|r| (r.stream_epoch, r.last_seq)))
}

pub async fn compute_backlog(
    pool: &PgPool,
    stream_id: Uuid,
    active_receiver_ids: &[String],
) -> Result<i64, sqlx::Error> {
    if active_receiver_ids.is_empty() {
        return Ok(0);
    }
    let total: i64 = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM events WHERE stream_id = $1",
        stream_id
    )
    .fetch_one(pool)
    .await?
    .unwrap_or(0);
    let mut min_acked = total;
    for receiver_id in active_receiver_ids {
        let cursor = fetch_cursor(pool, receiver_id, stream_id).await?;
        let acked = if let Some((epoch, seq)) = cursor {
            sqlx::query_scalar!(
                r#"SELECT COUNT(*) FROM events WHERE stream_id = $1 AND (stream_epoch < $2 OR (stream_epoch = $2 AND seq <= $3))"#,
                stream_id, epoch, seq
            ).fetch_one(pool).await?.unwrap_or(0)
        } else {
            0
        };
        if acked < min_acked {
            min_acked = acked;
        }
    }
    Ok(total - min_acked)
}
