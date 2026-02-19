//! Integration tests for dashboard HTTP API: streams, rename, metrics, reset-epoch.
use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
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

fn make_lazy_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://postgres:postgres@127.0.0.1:5432/postgres")
        .expect("lazy pool")
}

async fn make_server_with_dashboard(dashboard_dir: PathBuf) -> std::net::SocketAddr {
    let app_state = server::AppState::new(make_lazy_pool());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            server::build_router(app_state, Some(dashboard_dir)),
        )
        .await
        .unwrap();
    });
    addr
}

fn write_dashboard_fixture() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rt-dashboard-fixture-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create dashboard fixture dir");
    std::fs::write(
        dir.join("index.html"),
        "<!doctype html><html><body>dashboard-marker</body></html>",
    )
    .expect("write dashboard index");
    std::fs::write(dir.join("app.js"), "console.log('dashboard')").expect("write dashboard asset");
    dir
}

#[tokio::test]
async fn test_list_streams_empty() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["streams"].is_array(),
        "response must have 'streams' array"
    );
    assert_eq!(body["streams"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_list_streams_after_forwarder_connect() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(&pool, "fwd-list", "forwarder", b"fwd-list-token").await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-list-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-list".to_owned(),
        reader_ips: vec!["10.10.0.1:10000".to_owned()],
        resume: vec![],
        display_name: None,
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let streams = body["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    let s = &streams[0];
    assert_eq!(s["forwarder_id"], "fwd-list");
    assert_eq!(s["reader_ip"], "10.10.0.1:10000");
    assert!(s["stream_id"].is_string());
    assert!(
        s["online"].as_bool().unwrap_or(false),
        "stream should be online while forwarder connected"
    );
    assert!(s["stream_epoch"].is_number());
}

#[tokio::test]
async fn test_patch_stream_rename() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(&pool, "fwd-rename", "forwarder", b"fwd-rename-token").await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-rename-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-rename".to_owned(),
        reader_ips: vec!["10.20.0.1:10000".to_owned()],
        resume: vec![],
        display_name: None,
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    // Get stream_id
    let resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    let body: serde_json::Value = resp.json().await.unwrap();
    let stream_id = body["streams"][0]["stream_id"].as_str().unwrap().to_owned();

    // Rename
    let client = reqwest::Client::new();
    let patch_resp = client
        .patch(format!("http://{}/api/v1/streams/{}", addr, stream_id))
        .json(&serde_json::json!({ "display_alias": "Start Line" }))
        .send()
        .await
        .unwrap();
    assert_eq!(patch_resp.status(), 200);
    let patch_body: serde_json::Value = patch_resp.json().await.unwrap();
    assert_eq!(patch_body["display_alias"], "Start Line");

    // Verify persisted
    let resp2 = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    let body2: serde_json::Value = resp2.json().await.unwrap();
    assert_eq!(body2["streams"][0]["display_alias"], "Start Line");
}

#[tokio::test]
async fn test_patch_stream_not_found() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let client = reqwest::Client::new();
    let fake_id = "00000000-0000-0000-0000-000000000000";
    let resp = client
        .patch(format!("http://{}/api/v1/streams/{}", addr, fake_id))
        .json(&serde_json::json!({ "display_alias": "Ghost" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["code"].is_string());
    assert_eq!(body["code"], "NOT_FOUND");
}

#[tokio::test]
async fn test_get_metrics_for_stream() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(&pool, "fwd-metrics", "forwarder", b"fwd-metrics-token").await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-metrics-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-metrics".to_owned(),
        reader_ips: vec!["10.30.0.1:10000".to_owned()],
        resume: vec![],
        display_name: None,
    }))
    .await
    .unwrap();
    let session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    // Send 2 events, retransmit 1
    for seq in 1..=2u64 {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session.clone(),
            batch_id: format!("b{}", seq),
            events: vec![ReadEvent {
                forwarder_id: "fwd-metrics".to_owned(),
                reader_ip: "10.30.0.1:10000".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_read_line: format!("LINE_{}", seq),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }
    // Retransmit seq=1
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: session.clone(),
        batch_id: "r1".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-metrics".to_owned(),
            reader_ip: "10.30.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_read_line: "LINE_1".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    // Get metrics
    let streams_resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    let streams_body: serde_json::Value = streams_resp.json().await.unwrap();
    let stream_id = streams_body["streams"][0]["stream_id"].as_str().unwrap();

    let metrics_resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/metrics",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(metrics_resp.status(), 200);
    let m: serde_json::Value = metrics_resp.json().await.unwrap();
    assert_eq!(m["raw_count"], 3i64, "raw=3 (2 unique + 1 retransmit)");
    assert_eq!(m["dedup_count"], 2i64);
    assert_eq!(m["retransmit_count"], 1i64);
    // invariant
    assert_eq!(
        m["raw_count"].as_i64().unwrap(),
        m["dedup_count"].as_i64().unwrap() + m["retransmit_count"].as_i64().unwrap()
    );
    assert_eq!(m["backlog"], 0i64, "no active receivers");
    assert_eq!(m["epoch_raw_count"], 3i64);
    assert_eq!(m["epoch_dedup_count"], 2i64);
    assert_eq!(m["epoch_retransmit_count"], 1i64);
    assert!(m["epoch_lag_ms"].is_number());
    assert!(m["epoch_last_received_at"].is_string());
    assert_eq!(m["unique_chips"], 0i64);
}

#[tokio::test]
async fn test_metrics_not_found() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let resp = reqwest::get(format!(
        "http://{}/api/v1/streams/00000000-0000-0000-0000-000000000000/metrics",
        addr
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "NOT_FOUND");
}

#[tokio::test]
async fn test_healthz_and_readyz() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let r1 = reqwest::get(format!("http://{}/healthz", addr))
        .await
        .unwrap();
    assert_eq!(r1.status(), 200);
    let r2 = reqwest::get(format!("http://{}/readyz", addr))
        .await
        .unwrap();
    assert_eq!(r2.status(), 200);
}

#[tokio::test]
async fn test_forwarder_display_name_in_streams_list() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(&pool, "fwd-dn", "forwarder", b"fwd-dn-token").await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-dn-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-dn".to_owned(),
        reader_ips: vec!["10.40.0.1".to_owned()],
        resume: vec![],
        display_name: Some("Start Line".to_owned()),
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let streams = body["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["forwarder_display_name"], "Start Line");
}

#[tokio::test]
async fn test_forwarder_display_name_on_event_batch_created_stream() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(&pool, "fwd-dn-batch", "forwarder", b"fwd-dn-batch-token").await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-dn-batch-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-dn-batch".to_owned(),
        reader_ips: vec![],
        resume: vec![],
        display_name: Some("Start Line".to_owned()),
    }))
    .await
    .unwrap();
    let session_id = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {:?}", other),
    };

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id,
        batch_id: "batch-1".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-dn-batch".to_owned(),
            reader_ip: "10.41.0.1".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-18T10:00:00.000Z".to_owned(),
            raw_read_line: "LINE_1".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    let ack = fwd.recv_message().await.unwrap();
    assert!(matches!(ack, WsMessage::ForwarderAck(_)));

    let resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let streams = body["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert_eq!(streams[0]["forwarder_display_name"], "Start Line");
}

#[tokio::test]
async fn test_forwarder_display_name_cleared_when_hello_omits_value() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(&pool, "fwd-dn-clear", "forwarder", b"fwd-dn-clear-token").await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-dn-clear-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-dn-clear".to_owned(),
        reader_ips: vec!["10.42.0.1".to_owned()],
        resume: vec![],
        display_name: Some("Start Line".to_owned()),
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-dn-clear".to_owned(),
        reader_ips: vec!["10.42.0.1".to_owned()],
        resume: vec![],
        display_name: None,
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    let resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let streams = body["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    assert!(
        streams[0]["forwarder_display_name"].is_null(),
        "display name should clear when hello omits display_name"
    );
}

#[tokio::test]
async fn test_forwarder_display_name_refreshes_all_forwarder_streams() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(
        &pool,
        "fwd-dn-refresh-all",
        "forwarder",
        b"fwd-dn-refresh-all-token",
    )
    .await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-dn-refresh-all-token")
        .await
        .unwrap();

    // Initial hello creates two streams with the same display name.
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-dn-refresh-all".to_owned(),
        reader_ips: vec!["10.43.0.1".to_owned(), "10.43.0.2".to_owned()],
        resume: vec![],
        display_name: Some("Start Line".to_owned()),
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    // Subsequent hello updates display name but only includes one reader IP.
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-dn-refresh-all".to_owned(),
        reader_ips: vec!["10.43.0.1".to_owned()],
        resume: vec![],
        display_name: Some("Finish Line".to_owned()),
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    let resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let streams = body["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);
    for stream in streams {
        assert_eq!(stream["forwarder_id"], "fwd-dn-refresh-all");
        assert_eq!(stream["forwarder_display_name"], "Finish Line");
    }
}

#[tokio::test]
async fn test_dashboard_fallback_unknown_api_path_stays_not_found() {
    let dashboard_dir = write_dashboard_fixture();
    let addr = make_server_with_dashboard(dashboard_dir.clone()).await;

    let resp = reqwest::get(format!("http://{}/api/v1/does-not-exist", addr))
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
    std::fs::remove_dir_all(dashboard_dir).ok();
}

#[tokio::test]
async fn test_dashboard_fallback_unknown_ws_path_stays_not_found() {
    let dashboard_dir = write_dashboard_fixture();
    let addr = make_server_with_dashboard(dashboard_dir.clone()).await;

    let resp = reqwest::get(format!("http://{}/ws/v1/does-not-exist", addr))
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
    std::fs::remove_dir_all(dashboard_dir).ok();
}

#[tokio::test]
async fn test_dashboard_fallback_route_like_path_serves_index_html() {
    let dashboard_dir = write_dashboard_fixture();
    let addr = make_server_with_dashboard(dashboard_dir.clone()).await;

    let resp = reqwest::get(format!("http://{}/settings", addr))
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("dashboard-marker"));
    std::fs::remove_dir_all(dashboard_dir).ok();
}

#[tokio::test]
async fn test_dashboard_fallback_ui_paths_reject_post_method() {
    let dashboard_dir = write_dashboard_fixture();
    let addr = make_server_with_dashboard(dashboard_dir.clone()).await;

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://{}/", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 405);
    std::fs::remove_dir_all(dashboard_dir).ok();
}
