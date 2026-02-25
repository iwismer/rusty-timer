//! Integration tests for export.txt endpoint.
use rt_protocol::*;
use rt_test_utils::MockWsClient;
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

#[tokio::test]
async fn test_export_raw_canonical_events_ordered() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(&pool, "fwd-export", "forwarder", b"fwd-export-token").await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-export-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-export".to_owned(),
        reader_ips: vec!["10.40.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    // Send 3 events + 1 retransmit (should not appear in export)
    for seq in 1..=3u64 {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session.clone(),
            batch_id: format!("b{}", seq),
            events: vec![ReadEvent {
                forwarder_id: "fwd-export".to_owned(),
                reader_ip: "10.40.0.1:10000".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_frame: format!("EXPORT_LINE_{}", seq).into_bytes(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }
    // Retransmit seq=2 - must NOT appear in export
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: session.clone(),
        batch_id: "r2".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-export".to_owned(),
            reader_ip: "10.40.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 2,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_frame: "EXPORT_LINE_2".as_bytes().to_vec(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    // Get stream_id
    let streams_body: serde_json::Value = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let stream_id = streams_body["streams"][0]["stream_id"].as_str().unwrap();

    // Export raw
    let raw_resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/export.txt",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(raw_resp.status(), 200);
    let body = raw_resp.text().await.unwrap();

    // Should have exactly 3 lines (canonical deduped only)
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "export.txt must have exactly 3 canonical lines, got:\n{}",
        body
    );
    assert_eq!(lines[0], "EXPORT_LINE_1");
    assert_eq!(lines[1], "EXPORT_LINE_2");
    assert_eq!(lines[2], "EXPORT_LINE_3");
}

#[tokio::test]
async fn test_export_epoch_raw_filters_to_requested_epoch() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(&pool, "fwd-epoch-txt", "forwarder", b"fwd-epoch-txt-token").await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-epoch-txt-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-epoch-txt".to_owned(),
        reader_ips: vec!["10.40.0.2:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    for (batch_id, stream_epoch, seq, line) in [
        ("b-epoch-1", 1_u64, 1_u64, "EPOCH_1_LINE_1"),
        ("b-epoch-2-1", 2_u64, 1_u64, "EPOCH_2_LINE_1"),
        ("b-epoch-2-2", 2_u64, 2_u64, "EPOCH_2_LINE_2"),
    ] {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session.clone(),
            batch_id: batch_id.to_owned(),
            events: vec![ReadEvent {
                forwarder_id: "fwd-epoch-txt".to_owned(),
                reader_ip: "10.40.0.2:10000".to_owned(),
                stream_epoch,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_frame: line.as_bytes().to_vec(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }

    // Retransmit in epoch 2 should not appear in canonical export.
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: session.clone(),
        batch_id: "r-epoch-2-1".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-epoch-txt".to_owned(),
            reader_ip: "10.40.0.2:10000".to_owned(),
            stream_epoch: 2,
            seq: 1,
            reader_timestamp: "2026-02-17T10:00:01.000Z".to_owned(),
            raw_frame: "EPOCH_2_LINE_1".as_bytes().to_vec(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let streams_body: serde_json::Value = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let stream_id = streams_body["streams"][0]["stream_id"].as_str().unwrap();

    let raw_resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/epochs/2/export.txt",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(raw_resp.status(), 200);
    let body = raw_resp.text().await.unwrap();

    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(
        lines,
        vec!["EPOCH_2_LINE_1", "EPOCH_2_LINE_2"],
        "epoch export.txt must include only canonical lines from requested epoch"
    );
}

#[tokio::test]
async fn test_export_raw_not_found() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let resp = reqwest::get(format!(
        "http://{}/api/v1/streams/00000000-0000-0000-0000-000000000000/export.txt",
        addr
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_export_epoch_raw_not_found() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool).await;

    let resp = reqwest::get(format!(
        "http://{}/api/v1/streams/00000000-0000-0000-0000-000000000000/epochs/1/export.txt",
        addr
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);
}
