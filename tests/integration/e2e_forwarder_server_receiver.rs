//! End-to-End Integration Harness: Forwarder -> Server -> Receiver pipeline.
//!
//! Tests the complete event delivery pipeline:
//!   1. Server is started in-process (using the server library directly).
//!   2. A PostgreSQL container is managed via testcontainers-rs.
//!   3. A mock forwarder client (MockWsClient) sends event batches.
//!   4. A mock receiver client (MockWsClient) subscribes and receives events.
//!   5. A headless real receiver runtime smoke path is verified.
//!
//! Requires Docker for the Postgres testcontainer.
//!
//! # Coverage
//! - Two-stream pipeline: events from two different reader IPs arrive at receiver.
//! - Protocol-level mock receiver client: broad contract check.
//! - Headless receiver smoke path: receiver lib Db open + integrity_check.

use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

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
/// Returns the local address the server is bound to.
async fn start_server(pool: sqlx::PgPool) -> std::net::SocketAddr {
    let state = server::AppState::new(pool);
    let router = server::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind server");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server error");
    });
    // Give the server a moment to start accepting connections.
    tokio::time::sleep(Duration::from_millis(20)).await;
    addr
}

/// Perform the forwarder hello handshake and return the session_id.
async fn forwarder_handshake(
    client: &mut MockWsClient,
    forwarder_id: &str,
    reader_ips: Vec<String>,
) -> String {
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: forwarder_id.to_owned(),
            reader_ips,
            resume: vec![],
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

/// Perform the receiver hello handshake with a resume cursor list.
/// Returns the session_id.
async fn receiver_handshake(
    client: &mut MockWsClient,
    receiver_id: &str,
    resume: Vec<ResumeCursor>,
) -> String {
    client
        .send_message(&WsMessage::ReceiverHello(ReceiverHello {
            receiver_id: receiver_id.to_owned(),
            resume,
        }))
        .await
        .unwrap();
    match client.recv_message().await.unwrap() {
        WsMessage::Heartbeat(hb) => {
            assert!(!hb.session_id.is_empty(), "session_id must not be empty");
            hb.session_id
        }
        other => panic!("expected Heartbeat after receiver hello, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Test: Single forwarder stream — mock receiver client (protocol-level lane).
// ---------------------------------------------------------------------------

/// E2E test: forwarder -> server -> mock receiver client (protocol-level lane).
///
/// 1. Start Postgres container + in-process server.
/// 2. Forwarder client sends one event on one reader stream.
/// 3. Server acks the batch.
/// 4. Mock receiver client subscribes (via hello with resume cursor at seq=0).
/// 5. Receiver client receives the event batch.
#[tokio::test]
async fn e2e_single_stream_forwarder_to_mock_receiver() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    // Insert tokens for forwarder and receiver.
    insert_token(&pool, "fwd-e2e-01", "forwarder", b"fwd-e2e-token-01").await;
    insert_token(&pool, "rcv-e2e-01", "receiver", b"rcv-e2e-token-01").await;

    let addr = start_server(pool.clone()).await;

    // --- Forwarder lane: send an event batch ---
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd_client = MockWsClient::connect_with_token(&fwd_url, "fwd-e2e-token-01")
        .await
        .unwrap();
    let fwd_session = forwarder_handshake(
        &mut fwd_client,
        "fwd-e2e-01",
        vec!["192.168.10.1".to_owned()],
    )
    .await;

    let event = ReadEvent {
        forwarder_id: "fwd-e2e-01".to_owned(),
        reader_ip: "192.168.10.1".to_owned(),
        stream_epoch: 1,
        seq: 1,
        reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
        raw_read_line: "09001234567890001 10:00:00.000 1".to_owned(),
        read_type: "RAW".to_owned(),
    };

    fwd_client
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_session.clone(),
            batch_id: "e2e-batch-01".to_owned(),
            events: vec![event],
        }))
        .await
        .unwrap();

    // Receive and validate ack.
    match fwd_client.recv_message().await.unwrap() {
        WsMessage::ForwarderAck(ack) => {
            assert_eq!(ack.session_id, fwd_session);
            assert_eq!(ack.entries.len(), 1);
            assert_eq!(ack.entries[0].forwarder_id, "fwd-e2e-01");
            assert_eq!(ack.entries[0].reader_ip, "192.168.10.1");
            assert_eq!(ack.entries[0].stream_epoch, 1);
            assert_eq!(ack.entries[0].last_seq, 1);
        }
        other => panic!("expected ForwarderAck, got {:?}", other),
    }

    // Confirm event is stored in DB.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "event should be stored in server DB");

    // --- Receiver lane (mock client): subscribe via hello resume cursor ---
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv_client = MockWsClient::connect_with_token(&rcv_url, "rcv-e2e-token-01")
        .await
        .unwrap();

    // Resume cursor at epoch=1, seq=0 means "replay from the beginning".
    let _rcv_session = receiver_handshake(
        &mut rcv_client,
        "rcv-e2e-01",
        vec![ResumeCursor {
            forwarder_id: "fwd-e2e-01".to_owned(),
            reader_ip: "192.168.10.1".to_owned(),
            stream_epoch: 1,
            last_seq: 0,
        }],
    )
    .await;

    // Receiver should receive a batch with the event.
    let rcv_msg = tokio::time::timeout(Duration::from_secs(5), rcv_client.recv_message())
        .await
        .expect("timed out waiting for receiver batch")
        .unwrap();

    match rcv_msg {
        WsMessage::ReceiverEventBatch(batch) => {
            assert!(
                !batch.events.is_empty(),
                "should receive at least one event"
            );
            assert_eq!(batch.events[0].forwarder_id, "fwd-e2e-01");
            assert_eq!(batch.events[0].reader_ip, "192.168.10.1");
            assert_eq!(batch.events[0].seq, 1);
        }
        other => panic!("expected ReceiverEventBatch, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Test: Two streams, two reader IPs, both received by a mock receiver client.
// ---------------------------------------------------------------------------

/// E2E test: two forwarder reader streams -> server -> mock receiver.
///
/// Validates that events from two distinct reader IPs (two streams on the same
/// forwarder) are both delivered to a subscribing receiver client.
#[tokio::test]
async fn e2e_two_stream_forwarder_to_mock_receiver() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-e2e-02", "forwarder", b"fwd-e2e-token-02").await;
    insert_token(&pool, "rcv-e2e-02", "receiver", b"rcv-e2e-token-02").await;

    let addr = start_server(pool.clone()).await;

    // Forwarder sends events on two reader streams.
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd_client = MockWsClient::connect_with_token(&fwd_url, "fwd-e2e-token-02")
        .await
        .unwrap();
    let fwd_session = forwarder_handshake(
        &mut fwd_client,
        "fwd-e2e-02",
        vec!["192.168.20.1".to_owned(), "192.168.20.2".to_owned()],
    )
    .await;

    // Send a batch with events from both reader IPs.
    let batch = ForwarderEventBatch {
        session_id: fwd_session.clone(),
        batch_id: "e2e-two-stream-batch".to_owned(),
        events: vec![
            ReadEvent {
                forwarder_id: "fwd-e2e-02".to_owned(),
                reader_ip: "192.168.20.1".to_owned(),
                stream_epoch: 1,
                seq: 1,
                reader_timestamp: "2026-02-17T11:00:00.000Z".to_owned(),
                raw_read_line: "09001000000000001 11:00:00.000 1".to_owned(),
                read_type: "RAW".to_owned(),
            },
            ReadEvent {
                forwarder_id: "fwd-e2e-02".to_owned(),
                reader_ip: "192.168.20.2".to_owned(),
                stream_epoch: 1,
                seq: 1,
                reader_timestamp: "2026-02-17T11:00:01.000Z".to_owned(),
                raw_read_line: "09001000000000002 11:00:01.000 1".to_owned(),
                read_type: "RAW".to_owned(),
            },
        ],
    };

    fwd_client
        .send_message(&WsMessage::ForwarderEventBatch(batch))
        .await
        .unwrap();

    // Expect ack with two entries (one per stream).
    match fwd_client.recv_message().await.unwrap() {
        WsMessage::ForwarderAck(ack) => {
            assert_eq!(ack.entries.len(), 2, "should ack both streams");
        }
        other => panic!("expected ForwarderAck, got {:?}", other),
    }

    // Confirm two events stored.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2, "both events should be stored");

    // Receiver subscribes to both streams via hello resume cursors.
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv_client = MockWsClient::connect_with_token(&rcv_url, "rcv-e2e-token-02")
        .await
        .unwrap();
    let _rcv_session = receiver_handshake(
        &mut rcv_client,
        "rcv-e2e-02",
        vec![
            ResumeCursor {
                forwarder_id: "fwd-e2e-02".to_owned(),
                reader_ip: "192.168.20.1".to_owned(),
                stream_epoch: 1,
                last_seq: 0,
            },
            ResumeCursor {
                forwarder_id: "fwd-e2e-02".to_owned(),
                reader_ip: "192.168.20.2".to_owned(),
                stream_epoch: 1,
                last_seq: 0,
            },
        ],
    )
    .await;

    // Collect event batches until we have events from both streams.
    let mut received_reader_ips = std::collections::HashSet::new();
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_secs(5), rcv_client.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                for ev in &batch.events {
                    received_reader_ips.insert(ev.reader_ip.clone());
                }
                if received_reader_ips.len() >= 2 {
                    break;
                }
            }
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            Ok(Ok(other)) => panic!("unexpected message: {:?}", other),
            Ok(Err(e)) => panic!("recv error: {}", e),
            Err(_) => break,
        }
    }

    assert!(
        received_reader_ips.contains("192.168.20.1"),
        "should receive events from 192.168.20.1"
    );
    assert!(
        received_reader_ips.contains("192.168.20.2"),
        "should receive events from 192.168.20.2"
    );
}

// ---------------------------------------------------------------------------
// Test: Headless real receiver smoke path.
// ---------------------------------------------------------------------------

/// Smoke test: headless real receiver runtime can open its SQLite DB and pass
/// the integrity check. This validates the receiver's local durability path
/// without requiring a full network stack in the headless receiver runtime.
///
/// The receiver library's Db::open + integrity_check forms the minimum "ready"
/// gate the real receiver checks at startup before reporting "ready".
#[tokio::test]
async fn e2e_headless_receiver_sqlite_smoke() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_path = dir.path().join("receiver_smoke.sqlite3");

    // Open the receiver DB via the library — this exercises the real durability path.
    let db = receiver::Db::open(&db_path).expect("receiver Db::open should succeed");
    db.integrity_check()
        .expect("integrity_check should pass on fresh DB");
}

// ---------------------------------------------------------------------------
// Test: Receiver subscribe mid-session (add stream after connection).
// ---------------------------------------------------------------------------

/// E2E test: receiver subscribes to a stream mid-session via receiver_subscribe.
///
/// 1. Forwarder sends an event on a stream.
/// 2. Receiver connects without subscribing initially (empty resume).
/// 3. Receiver sends receiver_subscribe for the stream.
/// 4. Receiver receives the event batch.
#[tokio::test]
async fn e2e_receiver_subscribe_mid_session() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-e2e-03", "forwarder", b"fwd-e2e-token-03").await;
    insert_token(&pool, "rcv-e2e-03", "receiver", b"rcv-e2e-token-03").await;

    let addr = start_server(pool.clone()).await;

    // Forwarder sends an event.
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd_client = MockWsClient::connect_with_token(&fwd_url, "fwd-e2e-token-03")
        .await
        .unwrap();
    let fwd_session = forwarder_handshake(
        &mut fwd_client,
        "fwd-e2e-03",
        vec!["192.168.30.1".to_owned()],
    )
    .await;
    fwd_client
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_session,
            batch_id: "e2e-batch-sub".to_owned(),
            events: vec![ReadEvent {
                forwarder_id: "fwd-e2e-03".to_owned(),
                reader_ip: "192.168.30.1".to_owned(),
                stream_epoch: 1,
                seq: 1,
                reader_timestamp: "2026-02-17T12:00:00.000Z".to_owned(),
                raw_read_line: "09001000000000003 12:00:00.000 1".to_owned(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
    // Drain ack.
    fwd_client.recv_message().await.unwrap();

    // Receiver connects with empty resume list.
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv_client = MockWsClient::connect_with_token(&rcv_url, "rcv-e2e-token-03")
        .await
        .unwrap();
    let rcv_session = receiver_handshake(&mut rcv_client, "rcv-e2e-03", vec![]).await;

    // Send receiver_subscribe mid-session with a stream reference.
    rcv_client
        .send_message(&WsMessage::ReceiverSubscribe(ReceiverSubscribe {
            session_id: rcv_session,
            streams: vec![StreamRef {
                forwarder_id: "fwd-e2e-03".to_owned(),
                reader_ip: "192.168.30.1".to_owned(),
            }],
        }))
        .await
        .unwrap();

    // Should receive the event batch.
    let msg = tokio::time::timeout(Duration::from_secs(5), rcv_client.recv_message())
        .await
        .expect("timed out waiting for event after subscribe")
        .unwrap();
    match msg {
        WsMessage::ReceiverEventBatch(batch) => {
            assert!(
                !batch.events.is_empty(),
                "should receive at least one event"
            );
            assert_eq!(batch.events[0].reader_ip, "192.168.30.1");
        }
        other => panic!("expected ReceiverEventBatch, got {:?}", other),
    }
}
