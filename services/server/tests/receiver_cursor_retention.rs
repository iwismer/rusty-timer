use server::repo::receiver_cursors;
use sqlx::{PgPool, Row};
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

async fn setup_db() -> (ContainerAsync<Postgres>, PgPool) {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    (container, pool)
}

async fn insert_stream(pool: &PgPool, stream_epoch: i64) -> Uuid {
    let stream_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, stream_epoch, online) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(stream_id)
    .bind("fwd-retain")
    .bind(format!("10.1.0.{stream_epoch}:10000"))
    .bind(stream_epoch)
    .bind(false)
    .execute(pool)
    .await
    .unwrap();
    stream_id
}

#[tokio::test]
async fn prune_stale_cursors_keeps_current_epoch_and_recent_rows() {
    let (_container, pool) = setup_db().await;
    let stream_id = insert_stream(&pool, 2).await;

    receiver_cursors::upsert_cursor(&pool, "receiver-a", stream_id, 1, 100)
        .await
        .unwrap();
    receiver_cursors::upsert_cursor(&pool, "receiver-a", stream_id, 2, 200)
        .await
        .unwrap();
    receiver_cursors::upsert_cursor(&pool, "receiver-b", stream_id, 1, 150)
        .await
        .unwrap();
    receiver_cursors::upsert_cursor(&pool, "receiver-c", stream_id, 3, 250)
        .await
        .unwrap();

    sqlx::query(
        "UPDATE receiver_cursors SET updated_at = now() - INTERVAL '31 days' WHERE receiver_id IN ('receiver-a', 'receiver-c')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let deleted = receiver_cursors::prune_stale_cursors(&pool).await.unwrap();
    assert_eq!(deleted, 2);

    let rows = sqlx::query(
        "SELECT receiver_id, stream_epoch FROM receiver_cursors WHERE stream_id = $1 ORDER BY receiver_id, stream_epoch",
    )
    .bind(stream_id)
    .fetch_all(&pool)
    .await
    .unwrap();

    let retained: Vec<(String, i64)> = rows
        .into_iter()
        .map(|row| (row.get("receiver_id"), row.get("stream_epoch")))
        .collect();

    assert_eq!(
        retained,
        vec![("receiver-a".to_owned(), 2), ("receiver-b".to_owned(), 1),]
    );
}
