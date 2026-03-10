//! Integration test: Reader connection status propagation.
//!
//! Verifies that reader connection status flows from forwarder through server
//! to the HTTP API (`GET /api/v1/streams`).
//!
//! Requires Docker for the Postgres testcontainer.

use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

// ---------------------------------------------------------------------------
// Harness helpers (same pattern as e2e_forwarder_server_receiver.rs)
// ---------------------------------------------------------------------------

async fn insert_token(pool: &sqlx::PgPool, device_id: &str, device_type: &str, raw_token: &[u8]) {
    let hash = Sha256::digest(raw_token);
    let hash_bytes: Vec<u8> = hash.as_slice().to_vec();
    sqlx::query(
        "INSERT INTO device_tokens (token_hash, device_type, device_id) VALUES ($1, $2, $3)",
    )
    .bind(hash_bytes)
    .bind(device_type)
    .bind(device_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn start_server(pool: sqlx::PgPool) -> std::net::SocketAddr {
    let state = server::AppState::new(pool);
    let router = server::build_router(state, None);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind server");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server error");
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    addr
}

async fn forwarder_handshake(
    client: &mut MockWsClient,
    forwarder_id: &str,
    reader_ips: Vec<String>,
) -> String {
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: forwarder_id.to_owned(),
            reader_ips,
            display_name: None,
        }))
        .await
        .unwrap();
    match client.recv_message().await.unwrap() {
        WsMessage::Heartbeat(hb) => {
            assert!(!hb.session_id.is_empty(), "session_id must not be empty");
            hb.session_id
        }
        other => panic!("expected Heartbeat after forwarder hello, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Helper: query GET /api/v1/streams and return JSON array
// ---------------------------------------------------------------------------

async fn get_streams(addr: std::net::SocketAddr) -> Vec<serde_json::Value> {
    let url = format!("http://{}/api/v1/streams", addr);
    let resp = reqwest::get(&url)
        .await
        .expect("GET /api/v1/streams failed");
    assert!(
        resp.status().is_success(),
        "expected 2xx from /api/v1/streams"
    );
    let body: serde_json::Value = resp.json().await.expect("failed to parse streams JSON");
    body["streams"]
        .as_array()
        .expect("expected 'streams' array in response")
        .clone()
}

/// Find the stream entry matching the given reader_ip.
fn find_stream<'a>(streams: &'a [serde_json::Value], reader_ip: &str) -> &'a serde_json::Value {
    streams
        .iter()
        .find(|s| s["reader_ip"].as_str() == Some(reader_ip))
        .unwrap_or_else(|| {
            panic!(
                "stream with reader_ip={} not found in {:?}",
                reader_ip, streams
            )
        })
}

// ---------------------------------------------------------------------------
// Polling helper
// ---------------------------------------------------------------------------

async fn poll_until<F, Fut>(mut f: F, timeout: Duration)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if f().await {
            return;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("poll_until timed out after {:?}", timeout);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ---------------------------------------------------------------------------
// Test: Reader status propagation through the pipeline
// ---------------------------------------------------------------------------

/// Integration test: reader connection status flows from forwarder to HTTP API.
///
/// 1. Forwarder connects, sends ForwarderHello with one reader IP.
/// 2. Sends ReaderStatusUpdate(connected=true), verifies API reflects it.
/// 3. Sends ReaderStatusUpdate(connected=false), verifies API reflects it.
/// 4. Disconnects forwarder WS, verifies both online and reader_connected are false.
#[tokio::test]
async fn reader_status_propagation_through_api() {
    // --- Setup: Postgres container + server ---
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-status-01", "forwarder", b"fwd-status-token-01").await;

    let addr = start_server(pool.clone()).await;

    // --- Step 1: Forwarder connects and sends hello ---
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd_client = MockWsClient::connect_with_token(&fwd_url, "fwd-status-token-01")
        .await
        .unwrap();
    let _fwd_session = forwarder_handshake(
        &mut fwd_client,
        "fwd-status-01",
        vec!["192.168.1.10:9999".to_owned()],
    )
    .await;

    // Poll until the server has registered the stream.
    poll_until(
        || async {
            let streams = get_streams(addr).await;
            streams
                .iter()
                .any(|s| s["reader_ip"].as_str() == Some("192.168.1.10:9999"))
        },
        Duration::from_secs(5),
    )
    .await;

    // Verify initial state: stream exists, online=true, reader_connected=false.
    let streams = get_streams(addr).await;
    let stream = find_stream(&streams, "192.168.1.10:9999");
    assert_eq!(
        stream["online"].as_bool(),
        Some(true),
        "forwarder should be online after hello"
    );
    assert_eq!(
        stream["reader_connected"].as_bool(),
        Some(false),
        "reader_connected should be false initially"
    );

    // --- Step 2: Send ReaderStatusUpdate(connected=true) ---
    fwd_client
        .send_message(&WsMessage::ReaderStatusUpdate(ReaderStatusUpdate {
            reader_ip: "192.168.1.10:9999".to_owned(),
            connected: true,
        }))
        .await
        .unwrap();

    poll_until(
        || async {
            let streams = get_streams(addr).await;
            find_stream(&streams, "192.168.1.10:9999")["reader_connected"].as_bool() == Some(true)
        },
        Duration::from_secs(5),
    )
    .await;

    // --- Step 3: Send ReaderStatusUpdate(connected=false) ---
    fwd_client
        .send_message(&WsMessage::ReaderStatusUpdate(ReaderStatusUpdate {
            reader_ip: "192.168.1.10:9999".to_owned(),
            connected: false,
        }))
        .await
        .unwrap();

    poll_until(
        || async {
            let streams = get_streams(addr).await;
            find_stream(&streams, "192.168.1.10:9999")["reader_connected"].as_bool() == Some(false)
        },
        Duration::from_secs(5),
    )
    .await;

    // --- Step 4: Disconnect forwarder, verify both online and reader_connected are false ---
    fwd_client.close().await.unwrap();

    // Poll until the server processes the disconnection.
    poll_until(
        || async {
            let streams = get_streams(addr).await;
            let stream = find_stream(&streams, "192.168.1.10:9999");
            stream["online"].as_bool() == Some(false)
                && stream["reader_connected"].as_bool() == Some(false)
        },
        Duration::from_secs(5),
    )
    .await;
}
