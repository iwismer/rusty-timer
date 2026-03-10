use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

pub async fn create_pool(database_url: &str) -> PgPool {
    let max_conn: u32 = std::env::var("DATABASE_POOL_SIZE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    PgPoolOptions::new()
        .max_connections(max_conn)
        .connect(database_url)
        .await
        .expect("failed to connect to Postgres")
}

pub async fn run_migrations(pool: &PgPool) {
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .expect("failed to run database migrations")
}

/// Clear stale `online` and `reader_connected` flags for all streams.
/// Called at server startup since no forwarders can be connected yet.
pub async fn reset_stream_connection_state_on_startup(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE streams
         SET online = false, reader_connected = false
         WHERE online = true OR reader_connected = true",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
