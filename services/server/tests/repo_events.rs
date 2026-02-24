use server::repo::events::{
    IngestResult, count_unique_chips, fetch_events_after_cursor_limited,
    fetch_events_after_cursor_through_cursor_limited, fetch_max_event_cursor,
    fetch_stream_snapshot, set_stream_online, upsert_event, upsert_stream,
};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

async fn test_pool() -> (testcontainers::ContainerAsync<Postgres>, sqlx::PgPool) {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    (container, pool)
}

#[tokio::test]
async fn upsert_event_distinguishes_insert_retransmit_and_integrity_conflict() {
    let (_container, pool) = test_pool().await;
    let stream_id = upsert_stream(&pool, "fwd-repo-1", "10.0.0.1:10000", None)
        .await
        .unwrap();

    let inserted = upsert_event(
        &pool,
        stream_id,
        1,
        1,
        "2026-02-20T10:00:00.000Z",
        b"raw-line-1",
        "RAW",
    )
    .await
    .unwrap();
    assert_eq!(inserted.ingest_result, IngestResult::Inserted);
    assert_eq!(inserted.epoch_advanced_to, None);

    let retransmit = upsert_event(
        &pool,
        stream_id,
        1,
        1,
        "2026-02-20T10:00:00.000Z",
        b"raw-line-1",
        "RAW",
    )
    .await
    .unwrap();
    assert_eq!(retransmit.ingest_result, IngestResult::Retransmit);
    assert_eq!(retransmit.epoch_advanced_to, None);

    let conflict = upsert_event(
        &pool,
        stream_id,
        1,
        1,
        "2026-02-20T10:00:00.000Z",
        b"raw-line-1-mutated",
        "RAW",
    )
    .await
    .unwrap();
    assert_eq!(conflict.ingest_result, IngestResult::IntegrityConflict);

    let stored_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored_count, 1);
}

#[tokio::test]
async fn upsert_event_advances_epoch_and_updates_stream_epoch() {
    let (_container, pool) = test_pool().await;
    let stream_id = upsert_stream(&pool, "fwd-repo-2", "10.0.0.2:10000", None)
        .await
        .unwrap();

    upsert_event(
        &pool,
        stream_id,
        1,
        1,
        "2026-02-20T10:00:00.000Z",
        b"epoch1-line",
        "RAW",
    )
    .await
    .unwrap();

    let advanced = upsert_event(
        &pool,
        stream_id,
        2,
        1,
        "2026-02-20T10:00:01.000Z",
        b"epoch2-line",
        "RAW",
    )
    .await
    .unwrap();

    assert_eq!(advanced.ingest_result, IngestResult::Inserted);
    assert_eq!(advanced.epoch_advanced_to, Some(2));

    let current_epoch: i64 =
        sqlx::query_scalar("SELECT stream_epoch FROM streams WHERE stream_id = $1")
            .bind(stream_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(current_epoch, 2);
}

#[tokio::test]
async fn cursor_queries_return_ordered_ranges_and_latest_cursor() {
    let (_container, pool) = test_pool().await;
    let stream_id = upsert_stream(&pool, "fwd-repo-3", "10.0.0.3:10000", None)
        .await
        .unwrap();

    upsert_event(
        &pool,
        stream_id,
        1,
        1,
        "2026-02-20T10:00:00.000Z",
        b"e1s1",
        "RAW",
    )
    .await
    .unwrap();
    upsert_event(
        &pool,
        stream_id,
        1,
        2,
        "2026-02-20T10:00:01.000Z",
        b"e1s2",
        "RAW",
    )
    .await
    .unwrap();
    upsert_event(
        &pool,
        stream_id,
        1,
        3,
        "2026-02-20T10:00:02.000Z",
        b"e1s3",
        "RAW",
    )
    .await
    .unwrap();
    upsert_event(
        &pool,
        stream_id,
        2,
        1,
        "2026-02-20T10:00:03.000Z",
        b"e2s1",
        "RAW",
    )
    .await
    .unwrap();
    upsert_event(
        &pool,
        stream_id,
        2,
        2,
        "2026-02-20T10:00:04.000Z",
        b"e2s2",
        "RAW",
    )
    .await
    .unwrap();

    let limited = fetch_events_after_cursor_limited(&pool, stream_id, 1, 1, 2)
        .await
        .unwrap();
    assert_eq!(limited.len(), 2);
    assert_eq!((limited[0].stream_epoch, limited[0].seq), (1, 2));
    assert_eq!((limited[1].stream_epoch, limited[1].seq), (1, 3));

    let through =
        fetch_events_after_cursor_through_cursor_limited(&pool, stream_id, 1, 1, 2, 1, 10)
            .await
            .unwrap();
    assert_eq!(through.len(), 3);
    assert_eq!((through[0].stream_epoch, through[0].seq), (1, 2));
    assert_eq!((through[1].stream_epoch, through[1].seq), (1, 3));
    assert_eq!((through[2].stream_epoch, through[2].seq), (2, 1));

    let max_cursor = fetch_max_event_cursor(&pool, stream_id).await.unwrap();
    assert_eq!(max_cursor, Some((2, 2)));
}

#[tokio::test]
async fn stream_upsert_is_idempotent_online_toggles_and_unique_chip_count_ignores_null_tag() {
    let (_container, pool) = test_pool().await;

    let stream_id_1 = upsert_stream(&pool, "fwd-repo-4", "10.0.0.4:10000", Some("Line A"))
        .await
        .unwrap();
    let stream_id_2 = upsert_stream(&pool, "fwd-repo-4", "10.0.0.4:10000", Some("Line B"))
        .await
        .unwrap();
    assert_eq!(stream_id_1, stream_id_2);

    set_stream_online(&pool, stream_id_1, true).await.unwrap();
    let snapshot = fetch_stream_snapshot(&pool, stream_id_1)
        .await
        .unwrap()
        .unwrap();
    assert!(snapshot.online);

    upsert_event(
        &pool,
        stream_id_1,
        1,
        1,
        "2026-02-20T10:00:00.000Z",
        b"aa400000000123450a2a01123018455927a7",
        "RAW",
    )
    .await
    .unwrap();
    upsert_event(
        &pool,
        stream_id_1,
        1,
        2,
        "2026-02-20T10:00:01.000Z",
        b"not-a-chip-line",
        "RAW",
    )
    .await
    .unwrap();

    let unique = count_unique_chips(&pool, stream_id_1, 1).await.unwrap();
    assert_eq!(unique, 1);
}
