//! Integration tests for reads endpoints.
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

async fn make_server(pool: sqlx::PgPool) -> std::net::SocketAddr {
    let app_state = server::AppState::new(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });
    addr
}

async fn insert_stream(pool: &sqlx::PgPool, forwarder_id: &str, reader_ip: &str) -> uuid::Uuid {
    sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO streams (forwarder_id, reader_ip, stream_epoch, online) VALUES ($1, $2, 1, true) RETURNING stream_id",
    )
    .bind(forwarder_id)
    .bind(reader_ip)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_event_at(
    pool: &sqlx::PgPool,
    stream_id: uuid::Uuid,
    epoch: i64,
    seq: i64,
    received_at: &str,
) {
    sqlx::query(
        "INSERT INTO events (stream_id, stream_epoch, seq, tag_id, raw_frame, read_type, received_at) VALUES ($1, $2, $3, $4, $5, $6, $7::timestamptz)",
    )
    .bind(stream_id)
    .bind(epoch)
    .bind(seq)
    .bind("chip-a")
    .bind(format!("LINE_e{}_s{}", epoch, seq).into_bytes())
    .bind("RAW")
    .bind(received_at)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn get_stream_reads_orders_ties_by_seq_for_stable_pagination() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let stream_id = insert_stream(&pool, "fwd-tie-order", "10.1.1.1:10000").await;
    let tied_ts = "2026-01-01T00:00:00Z";

    // Insert in reverse seq order to expose unstable tie ordering.
    insert_event_at(&pool, stream_id, 1, 2, tied_ts).await;
    insert_event_at(&pool, stream_id, 1, 1, tied_ts).await;

    let page_1 = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/reads?order=asc&dedup=none&limit=1&offset=0",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(page_1.status(), 200);
    let page_1_json: serde_json::Value = page_1.json().await.unwrap();
    assert_eq!(page_1_json["reads"][0]["seq"].as_i64(), Some(1));

    let page_2 = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/reads?order=asc&dedup=none&limit=1&offset=1",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(page_2.status(), 200);
    let page_2_json: serde_json::Value = page_2.json().await.unwrap();
    assert_eq!(page_2_json["reads"][0]["seq"].as_i64(), Some(2));
}

#[tokio::test]
async fn get_stream_reads_returns_500_when_forwarder_race_lookup_fails() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let stream_id = insert_stream(&pool, "fwd-race-err-stream", "10.2.2.2:10000").await;
    insert_event_at(&pool, stream_id, 1, 1, "2026-01-01T00:00:00Z").await;

    sqlx::query("DROP TABLE forwarder_races")
        .execute(&pool)
        .await
        .unwrap();

    let resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/reads",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 500);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"].as_str(), Some("INTERNAL_ERROR"));
}

#[tokio::test]
async fn get_forwarder_reads_returns_500_when_forwarder_race_lookup_fails() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let _stream_id = insert_stream(&pool, "fwd-race-err-forwarder", "10.3.3.3:10000").await;

    sqlx::query("DROP TABLE forwarder_races")
        .execute(&pool)
        .await
        .unwrap();

    let resp = reqwest::get(format!(
        "http://{}/api/v1/forwarders/{}/reads",
        addr, "fwd-race-err-forwarder"
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 500);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"].as_str(), Some("INTERNAL_ERROR"));
}
