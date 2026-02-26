use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct AnnouncerConfigRow {
    pub enabled: bool,
    pub enabled_until: Option<DateTime<Utc>>,
    pub selected_stream_ids: Vec<Uuid>,
    pub max_list_size: i32,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct AnnouncerConfigUpdate {
    pub enabled: bool,
    pub enabled_until: Option<DateTime<Utc>>,
    pub selected_stream_ids: Vec<Uuid>,
    pub max_list_size: i32,
}

pub async fn get_config(pool: &PgPool) -> Result<AnnouncerConfigRow, sqlx::Error> {
    let row = sqlx::query(
        "SELECT enabled, enabled_until, selected_stream_ids, max_list_size, updated_at
         FROM announcer_config
         WHERE id = 1",
    )
    .fetch_one(pool)
    .await?;

    Ok(AnnouncerConfigRow {
        enabled: row.get("enabled"),
        enabled_until: row.get("enabled_until"),
        selected_stream_ids: row.get("selected_stream_ids"),
        max_list_size: row.get("max_list_size"),
        updated_at: row.get("updated_at"),
    })
}

pub async fn set_config(
    pool: &PgPool,
    update: &AnnouncerConfigUpdate,
) -> Result<AnnouncerConfigRow, sqlx::Error> {
    let row = sqlx::query(
        "UPDATE announcer_config
         SET enabled = $1,
             enabled_until = $2,
             selected_stream_ids = $3,
             max_list_size = $4,
             updated_at = now()
         WHERE id = 1
         RETURNING enabled, enabled_until, selected_stream_ids, max_list_size, updated_at",
    )
    .bind(update.enabled)
    .bind(update.enabled_until)
    .bind(&update.selected_stream_ids)
    .bind(update.max_list_size)
    .fetch_one(pool)
    .await?;

    Ok(AnnouncerConfigRow {
        enabled: row.get("enabled"),
        enabled_until: row.get("enabled_until"),
        selected_stream_ids: row.get("selected_stream_ids"),
        max_list_size: row.get("max_list_size"),
        updated_at: row.get("updated_at"),
    })
}
