//! End-to-End Export Verification: Emulator -> Forwarder Uplink -> Server -> Export.
//!
//! Tests that the server's export.txt and export.csv endpoints produce correct
//! output after events flow through the pipeline:
//!   1. A TCP emulator serves known IPICO reads from a fixture file.
//!   2. The test reads from the emulator, parses with ipico-core, and builds ReadEvents.
//!   3. The forwarder's UplinkSession sends events to an in-process server.
//!   4. The test hits the HTTP export endpoints and asserts exact output.
//!
//! Requires Docker for the Postgres testcontainer.

use forwarder::uplink::{SendBatchResult, UplinkConfig, UplinkSession};
use ipico_core::read::ChipRead;
use rt_protocol::ReadEvent;
use sha2::{Digest, Sha256};
use std::convert::TryFrom;
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Test fixture data
// ---------------------------------------------------------------------------

const FIXTURE_PATH: &str = "tests/test_assets/export_reads.txt";
const READER_IP: &str = "192.168.50.1";
const FORWARDER_DEVICE_ID: &str = "fwd-export-01";
const FORWARDER_TOKEN: &[u8] = b"fwd-export-token-01";
const READ_TYPE: &str = "RAW";

// ---------------------------------------------------------------------------
// Harness helpers
// ---------------------------------------------------------------------------

/// Insert a device token into the server DB for testing.
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

/// Spin up an in-process server against the given Postgres pool.
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

/// Start a TCP emulator that sends each read line (with \r\n) to every
/// connecting client, then keeps the connection open.
async fn start_tcp_emulator(reads: Vec<String>) -> (std::net::SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind emulator");
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("emulator accept failed");
        for read in &reads {
            let line = format!("{}\r\n", read);
            if stream.write_all(line.as_bytes()).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Keep connection open so the reader doesn't see EOF immediately.
        tokio::time::sleep(Duration::from_secs(30)).await;
    });
    (addr, handle)
}

/// Load reads from the fixture file.
fn load_fixture_reads() -> Vec<String> {
    let content = std::fs::read_to_string(FIXTURE_PATH).expect("failed to read fixture file");
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_owned())
        .collect()
}

/// Connect to the TCP emulator, read lines, parse as ChipRead, and build ReadEvents.
async fn read_events_from_emulator(
    emulator_addr: std::net::SocketAddr,
    expected_count: usize,
) -> Vec<ReadEvent> {
    let stream = tokio::net::TcpStream::connect(emulator_addr)
        .await
        .expect("failed to connect to emulator");
    let mut reader = BufReader::new(stream);
    let mut events = Vec::with_capacity(expected_count);

    for seq in 1..=expected_count {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .await
            .expect("failed to read line from emulator");
        let raw_line = line.trim_end_matches(['\r', '\n']).to_owned();

        let chip = ChipRead::try_from(raw_line.as_str())
            .unwrap_or_else(|e| panic!("failed to parse read '{}': {}", raw_line, e));

        events.push(ReadEvent {
            forwarder_id: FORWARDER_DEVICE_ID.to_owned(),
            reader_ip: READER_IP.to_owned(),
            stream_epoch: 1,
            seq: seq as u64,
            reader_timestamp: chip.timestamp.to_string(),
            raw_frame: raw_line.into_bytes(),
            read_type: READ_TYPE.to_owned(),
        });
    }

    events
}

// ---------------------------------------------------------------------------
// Test: Happy path — export.txt and export.csv content verification.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_export_txt_and_csv() {
    // --- Setup: Postgres + server ---
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, FORWARDER_DEVICE_ID, "forwarder", FORWARDER_TOKEN).await;

    let server_addr = start_server(pool.clone()).await;

    // --- Emulator: serve fixture reads over TCP ---
    let fixture_reads = load_fixture_reads();
    let read_count = fixture_reads.len();
    assert_eq!(read_count, 5, "fixture should contain exactly 5 reads");

    let (emulator_addr, emulator_handle) = start_tcp_emulator(fixture_reads.clone()).await;

    // --- Forwarder path: read from emulator, parse, send via UplinkSession ---
    let events = read_events_from_emulator(emulator_addr, read_count).await;

    let ws_url = format!("ws://{}/ws/v1/forwarders", server_addr);
    let uplink_cfg = UplinkConfig {
        server_url: ws_url,
        token: String::from_utf8_lossy(FORWARDER_TOKEN).to_string(),
        forwarder_id: FORWARDER_DEVICE_ID.to_owned(),
        display_name: None,
        batch_mode: "immediate".to_owned(),
        batch_flush_ms: 100,
        batch_max_events: 50,
    };
    let mut session = UplinkSession::connect_with_readers(uplink_cfg, vec![READER_IP.to_owned()])
        .await
        .expect("UplinkSession connect failed");

    match session.send_batch(events).await.expect("send_batch failed") {
        SendBatchResult::Ack(ack) => {
            assert_eq!(ack.entries.len(), 1);
            assert_eq!(ack.entries[0].last_seq, read_count as u64);
        }
        other => panic!("expected Ack, got {:?}", other),
    }

    // --- Wait for events in DB ---
    let mut attempts = 0;
    loop {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
            .fetch_one(&pool)
            .await
            .unwrap();
        if count >= read_count as i64 {
            break;
        }
        attempts += 1;
        assert!(attempts < 100, "timed out waiting for events in DB");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // --- Discover stream_id ---
    let http_base = format!("http://{}", server_addr);
    let client = reqwest::Client::new();

    let streams_resp: serde_json::Value = client
        .get(format!("{}/api/v1/streams", http_base))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let streams = streams_resp["streams"].as_array().expect("streams array");
    assert_eq!(streams.len(), 1, "should have exactly one stream");
    let stream_id = streams[0]["stream_id"].as_str().expect("stream_id string");

    // --- Verify export.txt ---
    let txt_resp = client
        .get(format!(
            "{}/api/v1/streams/{}/export.txt",
            http_base, stream_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(txt_resp.status(), 200);
    let txt_body = txt_resp.text().await.unwrap();

    let expected_txt = fixture_reads
        .iter()
        .map(|r| format!("{}\n", r))
        .collect::<String>();
    assert_eq!(txt_body, expected_txt, "export.txt content mismatch");

    // --- Verify export.csv ---
    let csv_resp = client
        .get(format!(
            "{}/api/v1/streams/{}/export.csv",
            http_base, stream_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(csv_resp.status(), 200);
    let csv_body = csv_resp.text().await.unwrap();

    let expected_timestamps = [
        "2001-12-30T18:45:00.000",
        "2001-12-30T18:45:10.100",
        "2001-12-30T18:45:20.250",
        "2001-12-30T18:45:30.500",
        "2001-12-30T18:45:40.990",
    ];

    let mut expected_csv =
        String::from("stream_epoch,seq,reader_timestamp,raw_frame,read_type,chip_id\n");
    for (i, read) in fixture_reads.iter().enumerate() {
        let chip_id = ChipRead::try_from(read.as_str())
            .unwrap_or_else(|e| panic!("failed to parse fixture read '{}': {}", read, e))
            .tag_id;
        expected_csv.push_str(&format!(
            "1,{},{},{},{},{}\n",
            i + 1,
            expected_timestamps[i],
            read,
            READ_TYPE,
            chip_id,
        ));
    }
    assert_eq!(csv_body, expected_csv, "export.csv content mismatch");

    // Cleanup
    emulator_handle.abort();
}

// ---------------------------------------------------------------------------
// Test: Non-existent stream returns 404.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn e2e_export_nonexistent_stream_returns_404() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    let server_addr = start_server(pool).await;
    let http_base = format!("http://{}", server_addr);
    let client = reqwest::Client::new();
    let fake_id = Uuid::new_v4();

    let txt_resp = client
        .get(format!(
            "{}/api/v1/streams/{}/export.txt",
            http_base, fake_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        txt_resp.status(),
        404,
        "export.txt should 404 for missing stream"
    );

    let csv_resp = client
        .get(format!(
            "{}/api/v1/streams/{}/export.csv",
            http_base, fake_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        csv_resp.status(),
        404,
        "export.csv should 404 for missing stream"
    );
}
