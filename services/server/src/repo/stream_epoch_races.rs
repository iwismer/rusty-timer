use sqlx::{PgPool, Row};
use uuid::Uuid;

pub struct StreamEpochRaceRow {
    pub stream_id: Uuid,
    pub stream_epoch: i64,
    pub race_id: Uuid,
}

pub async fn set_stream_epoch_race(
    pool: &PgPool,
    stream_id: Uuid,
    stream_epoch: i64,
    race_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    match race_id {
        Some(race_id) => {
            sqlx::query(
                "INSERT INTO stream_epoch_races (stream_id, stream_epoch, race_id) VALUES ($1, $2, $3)
                 ON CONFLICT (stream_id, stream_epoch) DO UPDATE
                 SET race_id = EXCLUDED.race_id",
            )
            .bind(stream_id)
            .bind(stream_epoch)
            .bind(race_id)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query(
                "DELETE FROM stream_epoch_races WHERE stream_id = $1 AND stream_epoch = $2",
            )
            .bind(stream_id)
            .bind(stream_epoch)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

pub async fn list_stream_epoch_races_by_race(
    pool: &PgPool,
    race_id: Uuid,
) -> Result<Vec<StreamEpochRaceRow>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT stream_id, stream_epoch, race_id
         FROM stream_epoch_races
         WHERE race_id = $1
         ORDER BY stream_id, stream_epoch",
    )
    .bind(race_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| StreamEpochRaceRow {
            stream_id: row.get("stream_id"),
            stream_epoch: row.get("stream_epoch"),
            race_id: row.get("race_id"),
        })
        .collect())
}

pub async fn list_mapped_epochs_by_stream(
    pool: &PgPool,
    stream_id: Uuid,
) -> Result<Vec<i64>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT stream_epoch
         FROM stream_epoch_races
         WHERE stream_id = $1
         ORDER BY stream_epoch",
    )
    .bind(stream_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| row.get("stream_epoch"))
        .collect())
}
