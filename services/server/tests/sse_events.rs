//! Integration test: SSE dashboard events are emitted when a forwarder connects and sends events.
use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

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

fn find_sse_event_data(collected: &str, event_name: &str) -> Option<String> {
    for block in collected.split("\n\n") {
        let mut name: Option<&str> = None;
        let mut data: Option<&str> = None;
        for line in block.lines() {
            if let Some(rest) = line.strip_prefix("event: ") {
                name = Some(rest);
            } else if let Some(rest) = line.strip_prefix("data: ") {
                data = Some(rest);
            }
        }
        if name == Some(event_name) {
            return data.map(ToOwned::to_owned);
        }
    }
    None
}

#[tokio::test]
async fn test_sse_emits_stream_created_and_metrics_updated() {
    // 1. Start Postgres container
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    // 2. Build state, start server on ephemeral port
    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state))
            .await
            .unwrap();
    });

    // 3. Insert a forwarder token
    insert_token(&pool, "fwd-sse", "forwarder", b"sse-test-token").await;

    // Seed an existing stream so StreamCreated should reflect persisted fields.
    let expected_stream_id = Uuid::new_v4();
    let expected_created_at = "2026-01-01T00:00:00+00:00";
    sqlx::query(
        "INSERT INTO streams (stream_id, forwarder_id, reader_ip, display_alias, stream_epoch, online, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7::timestamptz)",
    )
    .bind(expected_stream_id)
    .bind("fwd-sse")
    .bind("192.168.100.1:10000")
    .bind("Desk Reader")
    .bind(7_i64)
    .bind(false)
    .bind(expected_created_at)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO stream_metrics (stream_id) VALUES ($1)")
        .bind(expected_stream_id)
        .execute(&pool)
        .await
        .unwrap();

    // 4. Connect SSE client FIRST (before forwarder) so it doesn't miss events
    let sse_url = format!("http://{}/api/v1/events", addr);
    let mut sse_response = reqwest::Client::new().get(&sse_url).send().await.unwrap();
    assert_eq!(sse_response.status(), 200);

    // Give the SSE subscription a moment to be fully established
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 5. Connect forwarder via WebSocket
    let ws_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&ws_url, "sse-test-token")
        .await
        .unwrap();

    // 6. Send ForwarderHello with one reader_ip
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-sse".to_owned(),
        reader_ips: vec!["192.168.100.1:10000".to_owned()],
        resume: vec![],
        display_name: None,
    }))
    .await
    .unwrap();

    // 7. Wait for heartbeat response
    let session_id = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected Heartbeat, got {:?}", other),
    };

    // 8. Send a ForwarderEventBatch with one read event
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: session_id.clone(),
        batch_id: "b001".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-sse".to_owned(),
            reader_ip: "192.168.100.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-17T12:00:00.000Z".to_owned(),
            raw_read_line: "SSE_TEST_LINE".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();

    // 9. Wait for the ack
    match fwd.recv_message().await.unwrap() {
        WsMessage::ForwarderAck(a) => {
            assert_eq!(a.session_id, session_id);
            assert_eq!(a.entries.len(), 1);
        }
        other => panic!("expected ForwarderAck, got {:?}", other),
    }

    // 10. Read SSE chunks with a timeout, looking for stream_created and metrics_updated
    let mut collected = String::new();
    let mut saw_stream_created = false;
    let mut saw_metrics_updated = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);

    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), sse_response.chunk()).await {
            Ok(Ok(Some(chunk))) => {
                let text = String::from_utf8_lossy(&chunk);
                collected.push_str(&text);
                if collected.contains("event: stream_created") {
                    saw_stream_created = true;
                }
                if collected.contains("event: metrics_updated") {
                    saw_metrics_updated = true;
                }
                if saw_stream_created && saw_metrics_updated {
                    break;
                }
            }
            Ok(Ok(None)) => {
                // Stream ended unexpectedly
                break;
            }
            Ok(Err(e)) => {
                panic!("error reading SSE chunk: {:?}", e);
            }
            Err(_) => {
                // Timeout reading a chunk, check if we have what we need
                break;
            }
        }
    }

    // 11. Assert both events were received
    assert!(
        saw_stream_created,
        "expected 'event: stream_created' in SSE stream, got:\n{}",
        collected
    );
    assert!(
        saw_metrics_updated,
        "expected 'event: metrics_updated' in SSE stream, got:\n{}",
        collected
    );

    let stream_created_data = find_sse_event_data(&collected, "stream_created")
        .expect("missing stream_created data payload");
    let stream_created_json: serde_json::Value =
        serde_json::from_str(&stream_created_data).unwrap();
    let expected_stream_id_str = expected_stream_id.to_string();
    assert_eq!(
        stream_created_json["stream_id"].as_str(),
        Some(expected_stream_id_str.as_str())
    );
    assert_eq!(
        stream_created_json["display_alias"].as_str(),
        Some("Desk Reader")
    );
    assert_eq!(stream_created_json["stream_epoch"].as_i64(), Some(7));
    assert_eq!(
        stream_created_json["created_at"].as_str(),
        Some(expected_created_at)
    );

    let metrics_updated_data = find_sse_event_data(&collected, "metrics_updated")
        .expect("missing metrics_updated payload");
    let metrics_updated_json: serde_json::Value =
        serde_json::from_str(&metrics_updated_data).unwrap();
    assert_eq!(metrics_updated_json["raw_count"].as_i64(), Some(1));
    assert_eq!(metrics_updated_json["dedup_count"].as_i64(), Some(1));
    assert_eq!(metrics_updated_json["retransmit_count"].as_i64(), Some(0));
    assert_eq!(metrics_updated_json["epoch_raw_count"].as_i64(), Some(0));
    assert_eq!(metrics_updated_json["epoch_dedup_count"].as_i64(), Some(0));
    assert_eq!(
        metrics_updated_json["epoch_retransmit_count"].as_i64(),
        Some(0)
    );
    assert_eq!(metrics_updated_json["epoch_lag_ms"].as_i64(), None);
    assert_eq!(
        metrics_updated_json["epoch_last_received_at"].as_str(),
        None
    );
    assert_eq!(metrics_updated_json["unique_chips"].as_i64(), Some(0));

    // Keep the container alive until the end of the test
    std::mem::forget(container);
}
