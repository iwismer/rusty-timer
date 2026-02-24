use server::repo::{receiver_cursors, stream_epoch_races};
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

async fn insert_stream(
    pool: &PgPool,
    forwarder_id: &str,
    reader_ip: &str,
    stream_epoch: i64,
) -> Uuid {
    let stream_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, stream_epoch, online) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(stream_id)
    .bind(forwarder_id)
    .bind(reader_ip)
    .bind(stream_epoch)
    .bind(false)
    .execute(pool)
    .await
    .unwrap();
    stream_id
}

async fn insert_race(pool: &PgPool, name: &str) -> Uuid {
    let race_id = Uuid::new_v4();
    sqlx::query("INSERT INTO races (race_id, name) VALUES ($1, $2)")
        .bind(race_id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    race_id
}

#[tokio::test]
async fn cursor_is_persisted_per_epoch() {
    let (_container, pool) = setup_db().await;
    let stream_id = insert_stream(&pool, "fwd-epoch", "10.0.0.1:10000", 2).await;

    receiver_cursors::upsert_cursor(&pool, "receiver-a", stream_id, 1, 10)
        .await
        .unwrap();
    receiver_cursors::upsert_cursor(&pool, "receiver-a", stream_id, 2, 5)
        .await
        .unwrap();
    receiver_cursors::upsert_cursor(&pool, "receiver-a", stream_id, 1, 11)
        .await
        .unwrap();

    let epoch_1 = receiver_cursors::fetch_cursor_for_epoch(&pool, "receiver-a", stream_id, 1)
        .await
        .unwrap();
    let epoch_2 = receiver_cursors::fetch_cursor_for_epoch(&pool, "receiver-a", stream_id, 2)
        .await
        .unwrap();
    assert_eq!(epoch_1, Some(11));
    assert_eq!(epoch_2, Some(5));

    let all = receiver_cursors::fetch_cursors_for_stream(&pool, "receiver-a", stream_id)
        .await
        .unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].stream_epoch, 1);
    assert_eq!(all[0].last_seq, 11);
    assert_eq!(all[1].stream_epoch, 2);
    assert_eq!(all[1].last_seq, 5);
}

#[tokio::test]
async fn stream_epoch_race_mapping_set_unset_and_list() {
    let (_container, pool) = setup_db().await;
    let stream_a = insert_stream(&pool, "fwd-map", "10.0.0.2:10000", 2).await;
    let stream_b = insert_stream(&pool, "fwd-map", "10.0.0.3:10000", 4).await;
    let race_1 = insert_race(&pool, "Race 1").await;
    let race_2 = insert_race(&pool, "Race 2").await;

    stream_epoch_races::set_stream_epoch_race(&pool, stream_a, 1, Some(race_1))
        .await
        .unwrap();
    stream_epoch_races::set_stream_epoch_race(&pool, stream_a, 2, Some(race_1))
        .await
        .unwrap();
    stream_epoch_races::set_stream_epoch_race(&pool, stream_b, 4, Some(race_2))
        .await
        .unwrap();

    let race_1_rows = stream_epoch_races::list_stream_epoch_races_by_race(&pool, race_1)
        .await
        .unwrap();
    assert_eq!(race_1_rows.len(), 2);
    assert_eq!(race_1_rows[0].stream_id, stream_a);
    assert_eq!(race_1_rows[0].stream_epoch, 1);
    assert_eq!(race_1_rows[0].race_id, race_1);
    assert_eq!(race_1_rows[1].stream_id, stream_a);
    assert_eq!(race_1_rows[1].stream_epoch, 2);
    assert_eq!(race_1_rows[1].race_id, race_1);

    let epochs_a = stream_epoch_races::list_mapped_epochs_by_stream(&pool, stream_a)
        .await
        .unwrap();
    let epochs_b = stream_epoch_races::list_mapped_epochs_by_stream(&pool, stream_b)
        .await
        .unwrap();
    assert_eq!(epochs_a, vec![1, 2]);
    assert_eq!(epochs_b, vec![4]);

    stream_epoch_races::set_stream_epoch_race(&pool, stream_a, 1, None)
        .await
        .unwrap();
    let epochs_a_after = stream_epoch_races::list_mapped_epochs_by_stream(&pool, stream_a)
        .await
        .unwrap();
    assert_eq!(epochs_a_after, vec![2]);

    let count: i64 = sqlx::query("SELECT COUNT(*) AS count FROM stream_epoch_races")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("count");
    assert_eq!(count, 2);
}
