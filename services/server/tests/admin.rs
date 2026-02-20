//! Integration tests for admin HTTP API endpoints.
use reqwest::Client;
use sha2::{Digest, Sha256};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

async fn insert_token(pool: &sqlx::PgPool, device_id: &str, device_type: &str, raw_token: &[u8]) {
    let hash = Sha256::digest(raw_token);
    sqlx::query!(
        "INSERT INTO device_tokens (token_hash, device_type, device_id) VALUES ($1, $2, $3)",
        hash.as_slice(),
        device_type,
        device_id
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_stream(pool: &sqlx::PgPool, forwarder_id: &str, reader_ip: &str) -> uuid::Uuid {
    sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO streams (forwarder_id, reader_ip) VALUES ($1, $2) RETURNING stream_id",
    )
    .bind(forwarder_id)
    .bind(reader_ip)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_event(pool: &sqlx::PgPool, stream_id: uuid::Uuid, epoch: i64, seq: i64) {
    sqlx::query(
        "INSERT INTO events (stream_id, stream_epoch, seq, raw_read_line, read_type) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(stream_id)
    .bind(epoch)
    .bind(seq)
    .bind(format!("LINE_e{}_s{}", epoch, seq))
    .bind("RAW")
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_metrics(pool: &sqlx::PgPool, stream_id: uuid::Uuid) {
    sqlx::query(
        "INSERT INTO stream_metrics (stream_id, raw_count, dedup_count, retransmit_count) VALUES ($1, 1, 1, 0)",
    )
    .bind(stream_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_cursor(pool: &sqlx::PgPool, receiver_id: &str, stream_id: uuid::Uuid) {
    sqlx::query(
        "INSERT INTO receiver_cursors (receiver_id, stream_id, stream_epoch, last_seq) VALUES ($1, $2, 1, 1)",
    )
    .bind(receiver_id)
    .bind(stream_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_cursor_at(
    pool: &sqlx::PgPool,
    receiver_id: &str,
    stream_id: uuid::Uuid,
    stream_epoch: i64,
    last_seq: i64,
) {
    sqlx::query(
        "INSERT INTO receiver_cursors (receiver_id, stream_id, stream_epoch, last_seq) VALUES ($1, $2, $3, $4)",
    )
    .bind(receiver_id)
    .bind(stream_id)
    .bind(stream_epoch)
    .bind(last_seq)
    .execute(pool)
    .await
    .unwrap();
}

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

// ---------------------------------------------------------------------------
// Token tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_list_tokens_empty() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let resp = reqwest::get(format!("http://{}/api/v1/admin/tokens", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["tokens"].is_array(),
        "response must have 'tokens' array"
    );
    assert_eq!(body["tokens"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_list_tokens_returns_tokens() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-admin-1", "forwarder", b"admin-token-1").await;
    let addr = make_server(pool).await;

    let resp = reqwest::get(format!("http://{}/api/v1/admin/tokens", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let tokens = body["tokens"].as_array().unwrap();
    assert_eq!(tokens.len(), 1);
    let t = &tokens[0];
    assert_eq!(t["device_id"], "fwd-admin-1");
    assert_eq!(t["device_type"], "forwarder");
    assert!(t["token_id"].is_string());
    assert!(t["created_at"].is_string());
    assert_eq!(t["revoked"], false);
}

#[tokio::test]
async fn test_revoke_token() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-revoke", "forwarder", b"revoke-token").await;
    let addr = make_server(pool).await;

    // Get token_id
    let list_resp = reqwest::get(format!("http://{}/api/v1/admin/tokens", addr))
        .await
        .unwrap();
    let list_body: serde_json::Value = list_resp.json().await.unwrap();
    let token_id = list_body["tokens"][0]["token_id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Revoke
    let client = Client::new();
    let revoke_resp = client
        .post(format!(
            "http://{}/api/v1/admin/tokens/{}/revoke",
            addr, token_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(revoke_resp.status(), 204);

    // Verify revoked=true in list
    let list_resp2 = reqwest::get(format!("http://{}/api/v1/admin/tokens", addr))
        .await
        .unwrap();
    let list_body2: serde_json::Value = list_resp2.json().await.unwrap();
    assert_eq!(list_body2["tokens"][0]["revoked"], true);
}

#[tokio::test]
async fn test_revoke_token_not_found() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let client = Client::new();
    let fake_id = "00000000-0000-0000-0000-000000000000";
    let resp = client
        .post(format!(
            "http://{}/api/v1/admin/tokens/{}/revoke",
            addr, fake_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_create_token_auto_generate() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://{}/api/v1/admin/tokens", addr))
        .json(&serde_json::json!({
            "device_id": "my-forwarder",
            "device_type": "forwarder"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["device_id"], "my-forwarder");
    assert_eq!(body["device_type"], "forwarder");
    assert!(body["token_id"].is_string());
    let raw_token = body["token"].as_str().unwrap();
    assert_eq!(
        raw_token.len(),
        43,
        "URL-safe base64 of 32 bytes = 43 chars"
    );

    // Verify token hash is stored in DB
    let hash = Sha256::digest(raw_token.as_bytes());
    let row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM device_tokens WHERE token_hash = $1")
            .bind(hash.as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row_count, 1);
}

#[tokio::test]
async fn test_create_token_with_provided_token() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://{}/api/v1/admin/tokens", addr))
        .json(&serde_json::json!({
            "device_id": "my-receiver",
            "device_type": "receiver",
            "token": "my-custom-token-string"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["token"], "my-custom-token-string");
    assert_eq!(body["device_type"], "receiver");

    // Verify the custom token's hash is in DB
    let hash = Sha256::digest(b"my-custom-token-string");
    let row_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM device_tokens WHERE token_hash = $1")
            .bind(hash.as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row_count, 1);
}

#[tokio::test]
async fn test_create_token_invalid_device_type() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://{}/api/v1/admin/tokens", addr))
        .json(&serde_json::json!({
            "device_id": "some-device",
            "device_type": "invalid"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "BAD_REQUEST");
}

#[tokio::test]
async fn test_create_token_empty_device_id() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let client = Client::new();
    let resp = client
        .post(format!("http://{}/api/v1/admin/tokens", addr))
        .json(&serde_json::json!({
            "device_id": "  ",
            "device_type": "forwarder"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "BAD_REQUEST");
}

#[tokio::test]
async fn test_create_token_appears_in_list() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let client = Client::new();
    // Create a token
    let create_resp = client
        .post(format!("http://{}/api/v1/admin/tokens", addr))
        .json(&serde_json::json!({
            "device_id": "list-check",
            "device_type": "forwarder"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(create_resp.status(), 201);

    // Verify it shows in list
    let list_resp = reqwest::get(format!("http://{}/api/v1/admin/tokens", addr))
        .await
        .unwrap();
    let list_body: serde_json::Value = list_resp.json().await.unwrap();
    let tokens = list_body["tokens"].as_array().unwrap();
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0]["device_id"], "list-check");
    assert_eq!(tokens[0]["revoked"], false);
}

// ---------------------------------------------------------------------------
// Stream deletion tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_delete_stream_cascades() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let stream_id = insert_stream(&pool, "fwd-del", "10.0.0.1:10000").await;
    insert_event(&pool, stream_id, 1, 1).await;
    insert_metrics(&pool, stream_id).await;
    insert_cursor(&pool, "rcv-1", stream_id).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .delete(format!(
            "http://{}/api/v1/admin/streams/{}",
            addr, stream_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    // Verify all related data is gone
    let stream_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM streams")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stream_count, 0);

    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_count, 0);

    let metrics_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stream_metrics")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(metrics_count, 0);

    let cursor_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM receiver_cursors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cursor_count, 0);
}

#[tokio::test]
async fn test_delete_stream_not_found() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let client = Client::new();
    let fake_id = "00000000-0000-0000-0000-000000000000";
    let resp = client
        .delete(format!("http://{}/api/v1/admin/streams/{}", addr, fake_id))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_delete_all_streams() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let s1 = insert_stream(&pool, "fwd-all-1", "10.0.0.1:10000").await;
    let s2 = insert_stream(&pool, "fwd-all-2", "10.0.0.2:10000").await;
    insert_event(&pool, s1, 1, 1).await;
    insert_event(&pool, s2, 1, 1).await;
    insert_metrics(&pool, s1).await;
    insert_metrics(&pool, s2).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .delete(format!("http://{}/api/v1/admin/streams", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let stream_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM streams")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stream_count, 0);

    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_count, 0);

    let metrics_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM stream_metrics")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(metrics_count, 0);
}

// ---------------------------------------------------------------------------
// Event deletion tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_delete_all_events() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let stream_id = insert_stream(&pool, "fwd-evt", "10.0.0.1:10000").await;
    insert_event(&pool, stream_id, 1, 1).await;
    insert_event(&pool, stream_id, 1, 2).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .delete(format!("http://{}/api/v1/admin/events", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_count, 0);
}

#[tokio::test]
async fn test_delete_all_events_clears_all_cursors() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let s1 = insert_stream(&pool, "fwd-evt-a", "10.0.0.1:10000").await;
    let s2 = insert_stream(&pool, "fwd-evt-b", "10.0.0.2:10000").await;
    insert_event(&pool, s1, 1, 1).await;
    insert_event(&pool, s2, 1, 1).await;
    insert_cursor_at(&pool, "rcv-a", s1, 1, 1).await;
    insert_cursor_at(&pool, "rcv-b", s2, 1, 1).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .delete(format!("http://{}/api/v1/admin/events", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_count, 0);

    let cursor_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM receiver_cursors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cursor_count, 0);
}

#[tokio::test]
async fn test_delete_stream_events() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let s1 = insert_stream(&pool, "fwd-se-1", "10.0.0.1:10000").await;
    let s2 = insert_stream(&pool, "fwd-se-2", "10.0.0.2:10000").await;
    insert_event(&pool, s1, 1, 1).await;
    insert_event(&pool, s2, 1, 1).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .delete(format!(
            "http://{}/api/v1/admin/streams/{}/events",
            addr, s1
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let s1_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE stream_id = $1")
        .bind(s1)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(s1_count, 0, "s1 events should be deleted");

    let s2_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE stream_id = $1")
        .bind(s2)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(s2_count, 1, "s2 events should remain");
}

#[tokio::test]
async fn test_delete_stream_events_clears_stream_cursors() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let s1 = insert_stream(&pool, "fwd-se-c1", "10.0.0.1:10000").await;
    let s2 = insert_stream(&pool, "fwd-se-c2", "10.0.0.2:10000").await;
    insert_event(&pool, s1, 1, 1).await;
    insert_event(&pool, s2, 1, 1).await;
    insert_cursor_at(&pool, "rcv-a", s1, 1, 1).await;
    insert_cursor_at(&pool, "rcv-b", s2, 1, 1).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .delete(format!(
            "http://{}/api/v1/admin/streams/{}/events",
            addr, s1
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let s1_cursor_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM receiver_cursors WHERE stream_id = $1")
            .bind(s1)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(s1_cursor_count, 0, "s1 cursors should be deleted");

    let s2_cursor_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM receiver_cursors WHERE stream_id = $1")
            .bind(s2)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(s2_cursor_count, 1, "s2 cursors should remain");
}

#[tokio::test]
async fn test_delete_epoch_events() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let stream_id = insert_stream(&pool, "fwd-epoch", "10.0.0.1:10000").await;
    insert_event(&pool, stream_id, 1, 1).await;
    insert_event(&pool, stream_id, 2, 1).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .delete(format!(
            "http://{}/api/v1/admin/streams/{}/epochs/1/events",
            addr, stream_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let epoch1_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE stream_id = $1 AND stream_epoch = $2",
    )
    .bind(stream_id)
    .bind(1i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(epoch1_count, 0, "epoch 1 events should be deleted");

    let epoch2_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE stream_id = $1 AND stream_epoch = $2",
    )
    .bind(stream_id)
    .bind(2i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(epoch2_count, 1, "epoch 2 events should remain");
}

#[tokio::test]
async fn test_delete_epoch_events_clears_stream_cursors() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let s1 = insert_stream(&pool, "fwd-epoch-c1", "10.0.0.1:10000").await;
    let s2 = insert_stream(&pool, "fwd-epoch-c2", "10.0.0.2:10000").await;
    insert_event(&pool, s1, 1, 1).await;
    insert_event(&pool, s1, 2, 1).await;
    insert_event(&pool, s2, 1, 1).await;
    insert_cursor_at(&pool, "rcv-a", s1, 1, 1).await;
    insert_cursor_at(&pool, "rcv-b", s1, 2, 1).await;
    insert_cursor_at(&pool, "rcv-c", s2, 1, 1).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .delete(format!(
            "http://{}/api/v1/admin/streams/{}/epochs/1/events",
            addr, s1
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let s1_cursor_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM receiver_cursors WHERE stream_id = $1")
            .bind(s1)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(s1_cursor_count, 0, "all s1 cursors should be deleted");

    let s2_cursor_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM receiver_cursors WHERE stream_id = $1")
            .bind(s2)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(s2_cursor_count, 1, "other stream cursors should remain");
}

#[tokio::test]
async fn test_delete_epoch_events_rejects_epoch_below_one() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let stream_id = insert_stream(&pool, "fwd-epoch-bad", "10.0.0.1:10000").await;
    insert_event(&pool, stream_id, 1, 1).await;
    let addr = make_server(pool).await;

    let client = Client::new();
    let resp = client
        .delete(format!(
            "http://{}/api/v1/admin/streams/{}/epochs/0/events",
            addr, stream_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "BAD_REQUEST");
}

// ---------------------------------------------------------------------------
// Cursor deletion tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_delete_all_cursors() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let stream_id = insert_stream(&pool, "fwd-cur", "10.0.0.1:10000").await;
    insert_cursor(&pool, "rcv-a", stream_id).await;
    insert_cursor(&pool, "rcv-b", stream_id).await;
    let addr = make_server(pool.clone()).await;

    let client = Client::new();
    let resp = client
        .delete(format!("http://{}/api/v1/admin/receiver-cursors", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);

    let cursor_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM receiver_cursors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(cursor_count, 0);
}
