//! Integration test: SSE dashboard events are emitted when a forwarder connects and sends events.
use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::time::Duration;
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
        reader_ips: vec!["192.168.100.1".to_owned()],
        resume: vec![],
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
            reader_ip: "192.168.100.1".to_owned(),
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

    // Keep the container alive until the end of the test
    std::mem::forget(container);
}
