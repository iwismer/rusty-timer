//! Integration tests for dashboard HTTP API: streams, rename, metrics, reset-epoch.
use rt_protocol::*;
use rt_test_utils::MockWsClient;
use server::announcer::AnnouncerInputEvent;
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
        "INSERT INTO events (stream_id, stream_epoch, seq, raw_frame, read_type) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(stream_id)
    .bind(epoch)
    .bind(seq)
    .bind(format!("LINE_e{}_s{}", epoch, seq).into_bytes())
    .bind("RAW")
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
async fn selected_stream_epoch_change_triggers_reset() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_state = app_state.clone();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(server_state, None))
            .await
            .unwrap();
    });

    insert_token(
        &pool,
        "fwd-reset-epoch-announcer",
        "forwarder",
        b"fwd-reset-epoch-announcer-token",
    )
    .await;
    let ws_url = format!("ws://{addr}/ws/v1/forwarders");
    let mut fwd = MockWsClient::connect_with_token(&ws_url, "fwd-reset-epoch-announcer-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-reset-epoch-announcer".to_owned(),
        reader_ips: vec!["10.88.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let _session_id = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {:?}", other),
    };

    let stream_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2",
    )
    .bind("fwd-reset-epoch-announcer")
    .bind("10.88.0.1:10000")
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE announcer_config SET enabled = true, selected_stream_ids = $1")
        .bind(vec![stream_id])
        .execute(&pool)
        .await
        .unwrap();

    {
        let mut runtime = app_state.announcer_runtime.write().await;
        let _ = runtime.ingest(
            AnnouncerInputEvent {
                stream_id,
                seq: 1,
                chip_id: "000000444444".to_owned(),
                bib: Some(44),
                display_name: "Runner 44".to_owned(),
                reader_timestamp: Some("10:00:00".to_owned()),
                received_at: chrono::Utc::now(),
            },
            25,
        );
        assert_eq!(runtime.finisher_count(), 1);
    }

    let reset_resp = reqwest::Client::new()
        .post(format!(
            "http://{addr}/api/v1/streams/{stream_id}/reset-epoch"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(reset_resp.status(), reqwest::StatusCode::NO_CONTENT);

    match fwd.recv_message().await.unwrap() {
        WsMessage::EpochResetCommand(cmd) => {
            assert_eq!(cmd.forwarder_id, "fwd-reset-epoch-announcer");
            assert_eq!(cmd.reader_ip, "10.88.0.1:10000");
        }
        other => panic!("expected epoch reset command, got {:?}", other),
    }

    let runtime = app_state.announcer_runtime.read().await;
    assert_eq!(runtime.finisher_count(), 0);
    assert!(runtime.rows().is_empty());
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
    assert!(s["current_epoch_name"].is_null());
}

#[tokio::test]
async fn test_list_streams_includes_current_epoch_name_when_present() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let stream_id = insert_stream(&pool, "fwd-epoch-name", "10.11.0.1:10000").await;
    sqlx::query("UPDATE streams SET stream_epoch = $1 WHERE stream_id = $2")
        .bind(7_i64)
        .bind(stream_id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO stream_epoch_metadata (stream_id, stream_epoch, name) VALUES ($1, $2, $3)",
    )
    .bind(stream_id)
    .bind(7_i64)
    .bind("Lap 7")
    .execute(&pool)
    .await
    .unwrap();

    let resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let streams = body["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 1);
    let stream = &streams[0];
    assert_eq!(stream["stream_epoch"], 7);
    assert_eq!(stream["current_epoch_name"], "Lap 7");
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
                raw_frame: format!("LINE_{}", seq).into_bytes(),
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
            raw_frame: "LINE_1".as_bytes().to_vec(),
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
            raw_frame: "LINE_1".as_bytes().to_vec(),
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
        display_name: Some("Start Line".to_owned()),
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-dn-clear".to_owned(),
        reader_ips: vec!["10.42.0.1".to_owned()],
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
async fn test_logs_endpoint_sanitizes_forwarder_display_name_log_entry() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(
        &pool,
        "fwd-dn-log-sanitize",
        "forwarder",
        b"fwd-dn-log-sanitize-token",
    )
    .await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-dn-log-sanitize-token")
        .await
        .unwrap();

    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-dn-log-sanitize".to_owned(),
        reader_ips: vec!["10.44.0.1".to_owned()],
        display_name: Some("Start Line".to_owned()),
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-dn-log-sanitize".to_owned(),
        reader_ips: vec!["10.44.0.1".to_owned()],
        display_name: Some("Line 1\n[ERROR] forged".to_owned()),
    }))
    .await
    .unwrap();
    let hb = fwd.recv_message().await.unwrap();
    assert!(matches!(hb, WsMessage::Heartbeat(_)));

    let resp = reqwest::get(format!("http://{}/api/v1/logs", addr))
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let entries = body["entries"].as_array().unwrap();

    let display_name_entry = entries
        .iter()
        .filter_map(serde_json::Value::as_str)
        .find(|entry| {
            entry.contains("forwarder \"fwd-dn-log-sanitize\" display name set to")
                && entry.contains("Line 1")
        })
        .expect("expected display-name log entry");

    assert!(!display_name_entry.contains('\n'));
    assert!(!display_name_entry.contains('\r'));
    assert!(display_name_entry.contains("Line 1 [ERROR] forged"));
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
async fn test_dashboard_fallback_healthz_subpath_stays_not_found() {
    let dashboard_dir = write_dashboard_fixture();
    let addr = make_server_with_dashboard(dashboard_dir.clone()).await;

    let resp = reqwest::get(format!("http://{}/healthz/extra", addr))
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
    std::fs::remove_dir_all(dashboard_dir).ok();
}

#[tokio::test]
async fn test_dashboard_fallback_readyz_subpath_stays_not_found() {
    let dashboard_dir = write_dashboard_fixture();
    let addr = make_server_with_dashboard(dashboard_dir.clone()).await;

    let resp = reqwest::get(format!("http://{}/readyz/extra", addr))
        .await
        .unwrap();

    assert_eq!(resp.status(), 404);
    std::fs::remove_dir_all(dashboard_dir).ok();
}

#[tokio::test]
async fn test_dashboard_fallback_metrics_subpath_stays_not_found() {
    let dashboard_dir = write_dashboard_fixture();
    let addr = make_server_with_dashboard(dashboard_dir.clone()).await;

    let resp = reqwest::get(format!("http://{}/metrics/extra", addr))
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

#[tokio::test]
async fn test_list_epochs_returns_epochs_with_metadata() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let stream_id = insert_stream(&pool, "fwd-epochs", "10.50.0.1:10000").await;

    // Epoch 1: 2 events
    insert_event(&pool, stream_id, 1, 1).await;
    insert_event(&pool, stream_id, 1, 2).await;
    // Epoch 2: 1 event
    insert_event(&pool, stream_id, 2, 1).await;

    let resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/epochs",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let epochs = body.as_array().expect("response must be an array");
    assert_eq!(epochs.len(), 2, "should have 2 epochs");

    // Find epoch 1 and epoch 2
    let e1 = epochs.iter().find(|e| e["epoch"] == 1).expect("epoch 1");
    let e2 = epochs.iter().find(|e| e["epoch"] == 2).expect("epoch 2");

    assert_eq!(e1["event_count"], 2);
    assert_eq!(
        e1["is_current"], true,
        "epoch 1 is the stream default epoch"
    );
    assert!(e1["first_event_at"].is_string());
    assert!(e1["last_event_at"].is_string());
    assert!(e1["name"].is_null());

    assert_eq!(e2["event_count"], 1);
    assert_eq!(e2["is_current"], false);
    assert!(e2["first_event_at"].is_string());
    assert!(e2["last_event_at"].is_string());
    assert!(e2["name"].is_null());
}

#[tokio::test]
async fn list_epochs_includes_mapped_epoch_without_events() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let stream_id = insert_stream(&pool, "fwd-epochs-mapped-empty", "10.52.0.1:10000").await;
    insert_event(&pool, stream_id, 1, 1).await;

    let race_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO races (name) VALUES ($1) RETURNING race_id",
    )
    .bind("Mapped Empty Epoch")
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO stream_epoch_races (stream_id, stream_epoch, race_id) VALUES ($1, $2, $3)",
    )
    .bind(stream_id)
    .bind(3_i64)
    .bind(race_id)
    .execute(&pool)
    .await
    .unwrap();

    let resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/epochs",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let epochs = body.as_array().expect("response must be an array");
    assert_eq!(epochs.len(), 2, "event-backed + mapped-empty epoch");

    let e1 = epochs.iter().find(|e| e["epoch"] == 1).expect("epoch 1");
    let e3 = epochs.iter().find(|e| e["epoch"] == 3).expect("epoch 3");

    assert_eq!(e1["event_count"], 1);
    assert!(e1["first_event_at"].is_string());
    assert!(e1["last_event_at"].is_string());
    assert_eq!(e1["is_current"], true);
    assert!(e1["name"].is_null());

    assert_eq!(e3["event_count"], 0);
    assert!(e3["first_event_at"].is_null());
    assert!(e3["last_event_at"].is_null());
    assert_eq!(e3["is_current"], false);
    assert!(e3["name"].is_null());
}

#[tokio::test]
async fn test_list_epochs_empty_stream() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let stream_id = insert_stream(&pool, "fwd-epochs-empty", "10.51.0.1:10000").await;

    let resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/epochs",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let epochs = body.as_array().expect("response must be an array");
    assert_eq!(epochs.len(), 0, "no events means no epochs");
}

#[tokio::test]
async fn test_list_epochs_not_found() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let fake_id = "00000000-0000-0000-0000-000000000000";
    let resp = reqwest::get(format!("http://{}/api/v1/streams/{}/epochs", addr, fake_id))
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "NOT_FOUND");
}

#[tokio::test]
async fn test_put_epoch_name_set_and_normalize() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let stream_id = insert_stream(&pool, "fwd-epochs-name", "10.60.0.1:10000").await;
    insert_event(&pool, stream_id, 2, 1).await;

    let client = reqwest::Client::new();
    let set_resp = client
        .put(format!(
            "http://{}/api/v1/streams/{}/epochs/{}/name",
            addr, stream_id, 2
        ))
        .json(&serde_json::json!({ "name": "  Lap One  " }))
        .send()
        .await
        .unwrap();
    assert_eq!(set_resp.status(), 200);
    let set_body: serde_json::Value = set_resp.json().await.unwrap();
    assert_eq!(set_body["name"], "Lap One");

    let list_resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/epochs",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(list_resp.status(), 200);
    let list_body: serde_json::Value = list_resp.json().await.unwrap();
    let epochs = list_body.as_array().expect("response must be an array");
    let e2 = epochs.iter().find(|e| e["epoch"] == 2).expect("epoch 2");
    assert_eq!(e2["name"], "Lap One");

    let clear_resp = client
        .put(format!(
            "http://{}/api/v1/streams/{}/epochs/{}/name",
            addr, stream_id, 2
        ))
        .json(&serde_json::json!({ "name": "   " }))
        .send()
        .await
        .unwrap();
    assert_eq!(clear_resp.status(), 200);
    let clear_body: serde_json::Value = clear_resp.json().await.unwrap();
    assert!(clear_body["name"].is_null());

    let metadata_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM stream_epoch_metadata WHERE stream_id = $1 AND stream_epoch = $2",
    )
    .bind(stream_id)
    .bind(2_i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(metadata_count, 0, "clear should remove durable metadata");
}

#[tokio::test]
async fn test_put_epoch_name_clear_removes_metadata_only_epoch_from_list() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let stream_id = insert_stream(&pool, "fwd-epochs-clear-phantom", "10.62.0.1:10000").await;
    let client = reqwest::Client::new();

    let set_resp = client
        .put(format!(
            "http://{}/api/v1/streams/{}/epochs/{}/name",
            addr, stream_id, 9
        ))
        .json(&serde_json::json!({ "name": "Lap 9" }))
        .send()
        .await
        .unwrap();
    assert_eq!(set_resp.status(), 200);

    let listed_after_set = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/epochs",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(listed_after_set.status(), 200);
    let listed_after_set_body: serde_json::Value = listed_after_set.json().await.unwrap();
    let epochs_after_set = listed_after_set_body
        .as_array()
        .expect("response must be an array");
    assert_eq!(
        epochs_after_set.len(),
        1,
        "metadata-backed epoch should be listed"
    );
    assert_eq!(epochs_after_set[0]["epoch"], 9);
    assert_eq!(epochs_after_set[0]["name"], "Lap 9");

    let clear_resp = client
        .put(format!(
            "http://{}/api/v1/streams/{}/epochs/{}/name",
            addr, stream_id, 9
        ))
        .json(&serde_json::json!({ "name": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(clear_resp.status(), 200);

    let listed_after_clear = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/epochs",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(listed_after_clear.status(), 200);
    let listed_after_clear_body: serde_json::Value = listed_after_clear.json().await.unwrap();
    let epochs_after_clear = listed_after_clear_body
        .as_array()
        .expect("response must be an array");
    assert_eq!(
        epochs_after_clear.len(),
        0,
        "clearing name should remove metadata-only epoch from list"
    );

    let metadata_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM stream_epoch_metadata WHERE stream_id = $1 AND stream_epoch = $2",
    )
    .bind(stream_id)
    .bind(9_i64)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(metadata_count, 0, "clear should delete metadata row");
}

#[tokio::test]
async fn test_put_epoch_name_not_found() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let client = reqwest::Client::new();
    let fake_stream_id = "00000000-0000-0000-0000-000000000000";
    let resp = client
        .put(format!(
            "http://{}/api/v1/streams/{}/epochs/{}/name",
            addr, fake_stream_id, 1
        ))
        .json(&serde_json::json!({ "name": "Lap 1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "NOT_FOUND");
}

#[tokio::test]
async fn test_put_epoch_name_invalid_payload_returns_bad_request() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    let stream_id = insert_stream(&pool, "fwd-epochs-invalid", "10.61.0.1:10000").await;
    let client = reqwest::Client::new();
    let resp = client
        .put(format!(
            "http://{}/api/v1/streams/{}/epochs/{}/name",
            addr, stream_id, 1
        ))
        .json(&serde_json::json!({ "name": 123 }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "BAD_REQUEST");
}
