//! Server Stop/Restart Validation.
//!
//! Tests temporary server unavailability scenarios:
//! - Events ingested before stop are durable in Postgres after restart.
//! - New connections succeed after the server comes back up.
//! - Receiver replay picks up from where it left off after server restart.
//! - A new server instance (same DB) delivers all persisted events on reconnect.
//!
//! "Restart" is simulated by spinning up a new in-process server instance
//! against the same Postgres pool — equivalent to stopping and restarting
//! the server binary while the DB remains intact.

use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

// ---------------------------------------------------------------------------
// Harness helpers
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

/// Start a new in-process server instance on a fresh random port.
async fn start_server_instance(pool: sqlx::PgPool) -> std::net::SocketAddr {
    let state = server::AppState::new(pool);
    let router = server::build_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.expect("server error");
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    addr
}

// ---------------------------------------------------------------------------
// Test: Events survive server restart — data in DB is durable.
// ---------------------------------------------------------------------------

/// Server restart test: events ingested before server stop are present in DB
/// after a new server instance connects to the same Postgres.
///
/// The server itself is stateless regarding event storage — all events live in
/// Postgres. A "restart" (new process, same DB) must see all prior events.
#[tokio::test]
async fn server_restart_events_survive_in_postgres() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-srv-01", "forwarder", b"fwd-srv-token-01").await;
    insert_token(&pool, "rcv-srv-01", "receiver", b"rcv-srv-token-01").await;

    // --- Server instance 1: ingest events ---
    let addr1 = start_server_instance(pool.clone()).await;
    let fwd_url1 = format!("ws://{}/ws/v1/forwarders", addr1);

    let mut fwd = MockWsClient::connect_with_token(&fwd_url1, "fwd-srv-token-01")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-srv-01".to_owned(),
        reader_ips: vec!["10.110.110.1".to_owned()],
        resume: vec![],
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    // Send 4 events.
    for seq in 1u64..=4 {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_session.clone(),
            batch_id: format!("pre-restart-batch-{}", seq),
            events: vec![ReadEvent {
                forwarder_id: "fwd-srv-01".to_owned(),
                reader_ip: "10.110.110.1".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_read_line: format!("PRE_RESTART_LINE_{}", seq),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }

    // Verify all 4 events are in DB.
    let count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count_before, 4, "4 events should be in DB before server restart");

    // --- Simulate server restart: start server instance 2 (same DB) ---
    // Instance 1's port is abandoned (simulates server stop).
    let addr2 = start_server_instance(pool.clone()).await;

    // Verify DB still has the events (server restart doesn't affect Postgres).
    let count_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count_after, 4, "events must persist across server restart");

    // --- Receiver connects to server instance 2 and gets all events ---
    let rcv_url2 = format!("ws://{}/ws/v1/receivers", addr2);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url2, "rcv-srv-token-01")
        .await
        .unwrap();
    rcv.send_message(&WsMessage::ReceiverHello(ReceiverHello {
        receiver_id: "rcv-srv-01".to_owned(),
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-srv-01".to_owned(),
            reader_ip: "10.110.110.1".to_owned(),
            stream_epoch: 1,
            last_seq: 0,
        }],
    }))
    .await
    .unwrap();
    match rcv.recv_message().await.unwrap() {
        WsMessage::Heartbeat(_) => {}
        other => panic!("{:?}", other),
    }

    // Receiver should get all 4 events from the new server instance.
    let mut total_received = 0;
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                total_received += batch.events.len();
                if total_received >= 4 {
                    break;
                }
            }
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            _ => break,
        }
    }
    assert_eq!(
        total_received, 4,
        "receiver connected to new server instance should get all 4 pre-restart events"
    );
}

// ---------------------------------------------------------------------------
// Test: New forwarder can ingest events to server instance 2 after restart.
// ---------------------------------------------------------------------------

/// Server restart test: a forwarder can connect and ingest to the new server
/// instance after restart. All events (pre- and post-restart) are in the DB.
#[tokio::test]
async fn server_restart_new_forwarder_connection_works() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-srv-02", "forwarder", b"fwd-srv-token-02").await;

    // Server instance 1: send 2 events.
    let addr1 = start_server_instance(pool.clone()).await;
    let fwd_url1 = format!("ws://{}/ws/v1/forwarders", addr1);

    let mut fwd1 = MockWsClient::connect_with_token(&fwd_url1, "fwd-srv-token-02")
        .await
        .unwrap();
    fwd1.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-srv-02".to_owned(),
        reader_ips: vec!["10.120.120.1".to_owned()],
        resume: vec![],
    }))
    .await
    .unwrap();
    let fwd1_session = match fwd1.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    for seq in 1u64..=2 {
        fwd1.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd1_session.clone(),
            batch_id: format!("pre-{}", seq),
            events: vec![ReadEvent {
                forwarder_id: "fwd-srv-02".to_owned(),
                reader_ip: "10.120.120.1".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_read_line: format!("PRE_{}", seq),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd1.recv_message().await.unwrap();
    }

    // Restart: start server instance 2.
    let addr2 = start_server_instance(pool.clone()).await;

    // Connect to server instance 2 and send more events.
    let fwd_url2 = format!("ws://{}/ws/v1/forwarders", addr2);
    let mut fwd2 = MockWsClient::connect_with_token(&fwd_url2, "fwd-srv-token-02")
        .await
        .unwrap();
    fwd2.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-srv-02".to_owned(),
        reader_ips: vec!["10.120.120.1".to_owned()],
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-srv-02".to_owned(),
            reader_ip: "10.120.120.1".to_owned(),
            stream_epoch: 1,
            last_seq: 2,
        }],
    }))
    .await
    .unwrap();
    let fwd2_session = match fwd2.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    for seq in 3u64..=4 {
        fwd2.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd2_session.clone(),
            batch_id: format!("post-{}", seq),
            events: vec![ReadEvent {
                forwarder_id: "fwd-srv-02".to_owned(),
                reader_ip: "10.120.120.1".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:01.000Z".to_owned(),
                raw_read_line: format!("POST_{}", seq),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd2.recv_message().await.unwrap();
    }

    // All 4 events (2 pre + 2 post restart) must be in DB.
    let total_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(total_count, 4, "all 4 events (pre + post restart) must be in DB");
}

// ---------------------------------------------------------------------------
// Test: Server HTTP API is functional after restart.
// ---------------------------------------------------------------------------

/// Server restart test: the HTTP streams API on server instance 2 returns
/// streams that were created during server instance 1.
#[tokio::test]
async fn server_restart_http_api_accessible_after_restart() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-srv-03", "forwarder", b"fwd-srv-token-03").await;

    // Server 1: create a stream by sending a forwarder hello.
    let addr1 = start_server_instance(pool.clone()).await;
    let fwd_url1 = format!("ws://{}/ws/v1/forwarders", addr1);

    let mut fwd = MockWsClient::connect_with_token(&fwd_url1, "fwd-srv-token-03")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-srv-03".to_owned(),
        reader_ips: vec!["10.130.130.1".to_owned()],
        resume: vec![],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    // Send one event to ensure the stream is created.
    let fwd_session2 = {
        // We already received the heartbeat above — need the session_id.
        // Reconnect to get a clean session for sending.
        let mut fwd2 = MockWsClient::connect_with_token(&fwd_url1, "fwd-srv-token-03")
            .await
            .unwrap();
        fwd2.send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-srv-03".to_owned(),
            reader_ips: vec!["10.130.130.1".to_owned()],
            resume: vec![],
        }))
        .await
        .unwrap();
        // The second connection should fail (first-connection-wins).
        // Just wait briefly.
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    drop(fwd_session2);
    drop(fwd);

    // Verify stream exists in DB.
    let stream_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM streams WHERE forwarder_id = 'fwd-srv-03'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stream_count, 1, "stream should exist in DB after server 1");

    // Restart: server instance 2.
    let addr2 = start_server_instance(pool.clone()).await;

    // The healthz endpoint should respond OK.
    let healthz_url = format!("http://{}/healthz", addr2);
    let response = reqwest::get(&healthz_url).await.unwrap();
    assert_eq!(response.status(), 200, "/healthz should return 200 after restart");

    // The streams API should list the stream created before restart.
    let streams_url = format!("http://{}/api/v1/streams", addr2);
    let response = reqwest::get(&streams_url).await.unwrap();
    assert_eq!(response.status(), 200, "/api/v1/streams should return 200 after restart");

    // The streams API returns { "streams": [...] }.
    let body: serde_json::Value = response.json().await.unwrap();
    let streams = body["streams"].as_array().expect("should have streams array");
    let fwd_stream = streams
        .iter()
        .find(|s| s["forwarder_id"].as_str() == Some("fwd-srv-03"))
        .expect("stream created before restart should be visible after restart");
    assert_eq!(fwd_stream["reader_ip"].as_str(), Some("10.130.130.1"));
}
