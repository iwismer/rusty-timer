//! Integration tests for export.csv endpoint.
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

#[tokio::test]
async fn test_export_csv_header_and_rows() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });

    insert_token(&pool, "fwd-csv", "forwarder", b"fwd-csv-token").await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-csv-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-csv".to_owned(),
        reader_ips: vec!["10.50.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    // Send 2 events
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: session.clone(),
        batch_id: "b1".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-csv".to_owned(),
            reader_ip: "10.50.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-17T10:00:01.000Z".to_owned(),
            raw_read_line: "CSV_LINE_1".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    // Second event with comma in raw_read_line to test CSV escaping
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: session.clone(),
        batch_id: "b2".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-csv".to_owned(),
            reader_ip: "10.50.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 2,
            reader_timestamp: "2026-02-17T10:00:02.000Z".to_owned(),
            raw_read_line: "CSV,WITH,COMMAS".to_owned(),
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

    // Export CSV
    let csv_resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/export.csv",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(csv_resp.status(), 200);
    let body = csv_resp.text().await.unwrap();

    let lines: Vec<&str> = body.lines().collect();
    assert!(
        lines.len() >= 3,
        "CSV should have header + 2 data rows, got {} lines:\n{}",
        lines.len(),
        body
    );
    // Check header
    assert_eq!(
        lines[0], "stream_epoch,seq,reader_timestamp,raw_read_line,read_type",
        "unexpected CSV header"
    );
    // Check first data row
    assert!(
        lines[1].contains("CSV_LINE_1"),
        "first row should contain CSV_LINE_1"
    );
    // Check that commas in data are properly quoted (RFC 4180)
    assert!(
        lines[2].contains("\"CSV,WITH,COMMAS\""),
        "comma-containing data must be quoted, got: {}",
        lines[2]
    );
}

#[tokio::test]
async fn test_export_csv_not_found() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let app_state = server::AppState::new(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });

    let resp = reqwest::get(format!(
        "http://{}/api/v1/streams/00000000-0000-0000-0000-000000000000/export.csv",
        addr
    ))
    .await
    .unwrap();
    assert_eq!(resp.status(), 404);
}
