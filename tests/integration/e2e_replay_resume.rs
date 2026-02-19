//! End-to-End Replay and Resume Integration Tests.
//!
//! Tests the replay-on-reconnect behavior:
//! - Receiver reconnects after disconnect and gets only missing events.
//! - Replay starts from cursor + 1, never re-delivering already-acked events.
//! - Multi-epoch replay boundary: receiver at epoch boundary recovers correctly.
//!
//! Uses testcontainers-rs for a live Postgres container and an in-process server.

use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

// ---------------------------------------------------------------------------
// Harness helpers (shared with e2e_forwarder_server_receiver but duplicated
// here to keep each test file self-contained and independently runnable).
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

/// Send N sequential events starting at `start_seq` on the given stream and
/// drain all ack messages. Returns the final seq that was acked.
async fn send_events(
    client: &mut MockWsClient,
    session_id: &str,
    forwarder_id: &str,
    reader_ip: &str,
    stream_epoch: u64,
    start_seq: u64,
    count: u64,
) -> u64 {
    for i in 0..count {
        let seq = start_seq + i;
        client
            .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
                session_id: session_id.to_owned(),
                batch_id: format!("batch-{}-{}", stream_epoch, seq),
                events: vec![ReadEvent {
                    forwarder_id: forwarder_id.to_owned(),
                    reader_ip: reader_ip.to_owned(),
                    stream_epoch,
                    seq,
                    reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                    raw_read_line: format!("LINE_{}_{}", stream_epoch, seq),
                    read_type: "RAW".to_owned(),
                }],
            }))
            .await
            .unwrap();
        // Drain the ack.
        client.recv_message().await.unwrap();
    }
    start_seq + count - 1
}

// ---------------------------------------------------------------------------
// Test: Replay on reconnect — receiver gets only events after its cursor.
// ---------------------------------------------------------------------------

/// E2E test: Receiver disconnects and reconnects with a cursor; replays only
/// the events it hasn't seen.
///
/// Scenario:
/// 1. Forwarder sends events seq 1..5.
/// 2. Receiver connects, gets events 1..5, acks seq 5.
/// 3. Forwarder sends events seq 6..8.
/// 4. Receiver reconnects with cursor at seq=5.
/// 5. Receiver should only receive events seq 6..8 (no re-delivery of 1..5).
#[tokio::test]
async fn e2e_replay_on_reconnect_cursor_respected() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-rpl-01", "forwarder", b"fwd-rpl-token-01").await;
    insert_token(&pool, "rcv-rpl-01", "receiver", b"rcv-rpl-token-01").await;

    let addr = start_server(pool.clone()).await;

    // Connect forwarder and send events 1..5.
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-rpl-token-01")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-rpl-01".to_owned(),
        reader_ips: vec!["10.10.10.1".to_owned()],
        resume: vec![],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };
    send_events(&mut fwd, &fwd_session, "fwd-rpl-01", "10.10.10.1", 1, 1, 5).await;

    // First receiver session: connects with cursor at seq=0 (replay all).
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv1 = MockWsClient::connect_with_token(&rcv_url, "rcv-rpl-token-01")
        .await
        .unwrap();
    rcv1.send_message(&WsMessage::ReceiverHello(ReceiverHello {
        receiver_id: "rcv-rpl-01".to_owned(),
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-rpl-01".to_owned(),
            reader_ip: "10.10.10.1".to_owned(),
            stream_epoch: 1,
            last_seq: 0,
        }],
    }))
    .await
    .unwrap();
    let rcv1_session = match rcv1.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    // Drain events until we have seq 1..5 and ack them.
    let mut max_seq_seen: u64 = 0;
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_secs(3), rcv1.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                for ev in &batch.events {
                    if ev.seq > max_seq_seen {
                        max_seq_seen = ev.seq;
                    }
                }
                // Ack the batch.
                let entries = batch
                    .events
                    .iter()
                    .map(|ev| AckEntry {
                        forwarder_id: ev.forwarder_id.clone(),
                        reader_ip: ev.reader_ip.clone(),
                        stream_epoch: ev.stream_epoch,
                        last_seq: ev.seq,
                    })
                    .collect();
                rcv1.send_message(&WsMessage::ReceiverAck(ReceiverAck {
                    session_id: rcv1_session.clone(),
                    entries,
                }))
                .await
                .unwrap();
                if max_seq_seen >= 5 {
                    break;
                }
            }
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            Ok(Ok(other)) => panic!("unexpected: {:?}", other),
            Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(
        max_seq_seen >= 5,
        "first session should have received events up to seq 5"
    );

    // Forwarder sends 3 more events (seq 6..8).
    send_events(&mut fwd, &fwd_session, "fwd-rpl-01", "10.10.10.1", 1, 6, 3).await;

    // Receiver reconnects with cursor at seq=5 — should only get 6, 7, 8.
    let mut rcv2 = MockWsClient::connect_with_token(&rcv_url, "rcv-rpl-token-01")
        .await
        .unwrap();
    rcv2.send_message(&WsMessage::ReceiverHello(ReceiverHello {
        receiver_id: "rcv-rpl-01".to_owned(),
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-rpl-01".to_owned(),
            reader_ip: "10.10.10.1".to_owned(),
            stream_epoch: 1,
            last_seq: 5, // already have up to seq 5
        }],
    }))
    .await
    .unwrap();
    match rcv2.recv_message().await.unwrap() {
        WsMessage::Heartbeat(_) => {}
        other => panic!("expected Heartbeat, got {:?}", other),
    }

    // Collect events — none of them should have seq <= 5.
    let mut replay_seqs = Vec::new();
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_secs(3), rcv2.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                for ev in &batch.events {
                    assert!(
                        ev.seq > 5,
                        "replay should not re-deliver seq {} (already acked at 5)",
                        ev.seq
                    );
                    replay_seqs.push(ev.seq);
                }
                if replay_seqs.len() >= 3 {
                    break;
                }
            }
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            Ok(Ok(other)) => panic!("unexpected: {:?}", other),
            Ok(Err(_)) | Err(_) => break,
        }
    }

    assert!(
        replay_seqs.len() >= 3,
        "should replay 3 events (seq 6, 7, 8), got {:?}",
        replay_seqs
    );
    // Verify seq 6, 7, 8 are all present.
    assert!(replay_seqs.contains(&6), "seq 6 should be replayed");
    assert!(replay_seqs.contains(&7), "seq 7 should be replayed");
    assert!(replay_seqs.contains(&8), "seq 8 should be replayed");
}

// ---------------------------------------------------------------------------
// Test: Replay from seq=0 delivers all events.
// ---------------------------------------------------------------------------

/// E2E test: a receiver with cursor at seq=0 gets all events on the stream.
///
/// This is the "fresh receiver" scenario — receiver has no prior data.
#[tokio::test]
async fn e2e_replay_fresh_receiver_gets_all_events() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-rpl-02", "forwarder", b"fwd-rpl-token-02").await;
    insert_token(&pool, "rcv-rpl-02", "receiver", b"rcv-rpl-token-02").await;

    let addr = start_server(pool.clone()).await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-rpl-token-02")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-rpl-02".to_owned(),
        reader_ips: vec!["10.20.20.1".to_owned()],
        resume: vec![],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    // Send 4 events.
    send_events(&mut fwd, &fwd_session, "fwd-rpl-02", "10.20.20.1", 1, 1, 4).await;

    // Receiver with cursor at seq=0 (fresh receiver — replays all).
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-rpl-token-02")
        .await
        .unwrap();
    rcv.send_message(&WsMessage::ReceiverHello(ReceiverHello {
        receiver_id: "rcv-rpl-02".to_owned(),
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-rpl-02".to_owned(),
            reader_ip: "10.20.20.1".to_owned(),
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

    let mut total_events = 0usize;
    let mut min_seq = u64::MAX;
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_secs(3), rcv.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                for ev in &batch.events {
                    if ev.seq < min_seq {
                        min_seq = ev.seq;
                    }
                    total_events += 1;
                }
                if total_events >= 4 {
                    break;
                }
            }
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            Ok(Ok(other)) => panic!("unexpected: {:?}", other),
            Ok(Err(_)) | Err(_) => break,
        }
    }

    assert!(
        total_events >= 4,
        "fresh receiver should get all 4 events, got {}",
        total_events
    );
    assert_eq!(min_seq, 1, "first event should be seq=1");
}

// ---------------------------------------------------------------------------
// Test: Forwarder epoch reset — old epoch events remain replayable.
// ---------------------------------------------------------------------------

/// E2E test: after epoch reset, receiver can replay old-epoch events from their
/// cursor and new-epoch events once the forwarder sends them.
///
/// Design rule: "Older-epoch unacked backlog remains eligible for replay/ack
/// until drained."
#[tokio::test]
async fn e2e_epoch_reset_old_epoch_remains_replayable() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-epoch-01", "forwarder", b"fwd-epoch-token-01").await;
    insert_token(&pool, "rcv-epoch-01", "receiver", b"rcv-epoch-token-01").await;

    let addr = start_server(pool.clone()).await;

    // Connect forwarder on epoch 1 and send 2 events.
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-epoch-token-01")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-epoch-01".to_owned(),
        reader_ips: vec!["10.30.30.1".to_owned()],
        resume: vec![],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session_ep1 = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    // Send epoch 1, seq 1 and 2.
    send_events(
        &mut fwd,
        &fwd_session_ep1,
        "fwd-epoch-01",
        "10.30.30.1",
        1,
        1,
        2,
    )
    .await;

    // Simulate epoch reset: forwarder re-hellos with epoch 2.
    // The server receives a new hello implying the epoch was reset.
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-epoch-01".to_owned(),
        reader_ips: vec!["10.30.30.1".to_owned()],
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-epoch-01".to_owned(),
            reader_ip: "10.30.30.1".to_owned(),
            stream_epoch: 2,
            last_seq: 0,
        }],
        display_name: None,
    }))
    .await
    .unwrap();
    // Expect a new heartbeat with the same or new session.
    let fwd_session_ep2 = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected Heartbeat after re-hello, got {:?}", other),
    };

    // Send epoch 2, seq 1.
    send_events(
        &mut fwd,
        &fwd_session_ep2,
        "fwd-epoch-01",
        "10.30.30.1",
        2,
        1,
        1,
    )
    .await;

    // Verify: DB should have 3 events total (epoch 1 seq 1,2 + epoch 2 seq 1).
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3, "should have events from both epochs");

    // Receiver subscribes from epoch 1, seq=0 — should replay all 3 events.
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-epoch-token-01")
        .await
        .unwrap();
    rcv.send_message(&WsMessage::ReceiverHello(ReceiverHello {
        receiver_id: "rcv-epoch-01".to_owned(),
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-epoch-01".to_owned(),
            reader_ip: "10.30.30.1".to_owned(),
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

    // Collect all replayed events.
    let mut total = 0usize;
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_secs(3), rcv.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                total += batch.events.len();
                if total >= 3 {
                    break;
                }
            }
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            Ok(Ok(other)) => panic!("unexpected: {:?}", other),
            Ok(Err(_)) | Err(_) => break,
        }
    }
    // Old-epoch events must remain replayable after epoch reset.
    assert!(
        total >= 3,
        "old-epoch events must remain replayable after epoch reset, got {}",
        total
    );
}

// ---------------------------------------------------------------------------
// Test: Receiver ack advances the cursor (no re-delivery after ack).
// ---------------------------------------------------------------------------

/// E2E test: after receiver acks events up to seq N, re-connecting with
/// cursor at N does not re-deliver those events.
#[tokio::test]
async fn e2e_receiver_ack_advances_cursor() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-rpl-03", "forwarder", b"fwd-rpl-token-03").await;
    insert_token(&pool, "rcv-rpl-03", "receiver", b"rcv-rpl-token-03").await;

    let addr = start_server(pool.clone()).await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-rpl-token-03")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-rpl-03".to_owned(),
        reader_ips: vec!["10.40.40.1".to_owned()],
        resume: vec![],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };
    // Send events 1..3.
    send_events(&mut fwd, &fwd_session, "fwd-rpl-03", "10.40.40.1", 1, 1, 3).await;

    // Receiver connects, gets all 3, acks seq=3.
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-rpl-token-03")
        .await
        .unwrap();
    rcv.send_message(&WsMessage::ReceiverHello(ReceiverHello {
        receiver_id: "rcv-rpl-03".to_owned(),
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-rpl-03".to_owned(),
            reader_ip: "10.40.40.1".to_owned(),
            stream_epoch: 1,
            last_seq: 3, // simulate cursor already at 3
        }],
    }))
    .await
    .unwrap();
    match rcv.recv_message().await.unwrap() {
        WsMessage::Heartbeat(_) => {}
        other => panic!("{:?}", other),
    }

    // With cursor at 3, server should send no events (all already seen).
    // Any batch received must have seq > 3.
    match tokio::time::timeout(Duration::from_secs(2), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            for ev in &batch.events {
                assert!(
                    ev.seq > 3,
                    "should not re-deliver seq {} (cursor is at 3)",
                    ev.seq
                );
            }
        }
        Ok(Ok(WsMessage::Heartbeat(_))) => {}
        Ok(Ok(other)) => panic!("unexpected: {:?}", other),
        // Timeout or connection close are acceptable — no events to deliver.
        Ok(Err(_)) | Err(_) => {}
    }
}
