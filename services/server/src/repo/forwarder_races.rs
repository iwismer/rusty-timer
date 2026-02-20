use sqlx::{PgPool, Row};
use uuid::Uuid;

pub async fn get_forwarder_race(
    pool: &PgPool,
    forwarder_id: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row = sqlx::query("SELECT race_id FROM forwarder_races WHERE forwarder_id = $1")
        .bind(forwarder_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.get::<Option<Uuid>, _>("race_id")))
}

pub async fn set_forwarder_race(
    pool: &PgPool,
    forwarder_id: &str,
    race_id: Option<Uuid>,
) -> Result<(), sqlx::Error> {
    match race_id {
        Some(rid) => {
            sqlx::query(
                "INSERT INTO forwarder_races (forwarder_id, race_id) VALUES ($1, $2) ON CONFLICT (forwarder_id) DO UPDATE SET race_id = EXCLUDED.race_id",
            )
            .bind(forwarder_id)
            .bind(rid)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM forwarder_races WHERE forwarder_id = $1")
                .bind(forwarder_id)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub struct ForwarderRaceRow {
    pub forwarder_id: String,
    pub race_id: Option<Uuid>,
}

pub async fn list_forwarder_races(pool: &PgPool) -> Result<Vec<ForwarderRaceRow>, sqlx::Error> {
    let rows = sqlx::query("SELECT forwarder_id, race_id FROM forwarder_races")
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| ForwarderRaceRow {
            forwarder_id: r.get("forwarder_id"),
            race_id: r.get("race_id"),
        })
        .collect())
}
