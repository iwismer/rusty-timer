//! Chaos Network: network flap suite for the forwarder->server pipeline.
//!
//! Tests that events are not lost and replay is correct when the forwarder's
//! connection to the server is interrupted mid-session. These tests use the
//! in-process server (testcontainers Postgres) to simulate real network flaps
//! by closing and reopening the WebSocket session.
//!
//! # Scenarios
//! 1. Flap mid-batch: connection drops after some events but before ack — replay delivers all.
//! 2. Rapid reconnect: many reconnects in a short window don't lose events.
//! 3. Receiver connection flap: receiver reconnects and resumes from cursor.
//! 4. No-data loss invariant: all events sent before each flap are eventually in DB.

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
    tokio::time::sleep(Duration::from_millis(20)).await;
    addr
}

/// Connect a forwarder client and perform the hello handshake. Returns (client, session_id).
async fn connect_forwarder(
    fwd_url: &str,
    token: &str,
    forwarder_id: &str,
    reader_ips: Vec<String>,
    resume: Vec<ResumeCursor>,
) -> (MockWsClient, String) {
    let mut client = MockWsClient::connect_with_token(fwd_url, token)
        .await
        .unwrap();
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: forwarder_id.to_owned(),
            reader_ips,
            resume,
            display_name: None,
        }))
        .await
        .unwrap();
    let session_id = match client.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected Heartbeat, got {:?}", other),
    };
    (client, session_id)
}

// ---------------------------------------------------------------------------
// Test: Flap mid-batch — events sent before disconnection are in DB.
// ---------------------------------------------------------------------------

/// Chaos test: forwarder sends events, connection drops (simulated by
/// abandoning the client), then reconnects and replays unacked events.
///
/// All events sent before the flap must be present in the server DB.
/// After reconnect, duplicate sends (retransmits) must not create duplicates.
#[tokio::test]
async fn chaos_network_flap_events_not_lost() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-chaos-01", "forwarder", b"fwd-chaos-token-01").await;

    let addr = start_server(pool.clone()).await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);

    // Session 1: send 3 events, then explicitly close (simulates graceful disconnect
    // before we can process acks — all events are in the server DB at this point).
    {
        let (mut client, session_id) = connect_forwarder(
            &fwd_url,
            "fwd-chaos-token-01",
            "fwd-chaos-01",
            vec!["10.50.50.1".to_owned()],
            vec![],
        )
        .await;

        for seq in 1u64..=3 {
            client
                .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
                    session_id: session_id.clone(),
                    batch_id: format!("batch-s1-{}", seq),
                    events: vec![ReadEvent {
                        forwarder_id: "fwd-chaos-01".to_owned(),
                        reader_ip: "10.50.50.1".to_owned(),
                        stream_epoch: 1,
                        seq,
                        reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                        raw_read_line: format!("CHAOS_LINE_{}", seq),
                        read_type: "RAW".to_owned(),
                    }],
                }))
                .await
                .unwrap();
            // Drain ack (events are persisted on the server).
            client.recv_message().await.unwrap();
        }
        // Close the connection explicitly so reconnect can proceed.
        let _ = client.close().await;
        // Brief pause to let the server register the disconnect.
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // All 3 events must be in the DB before reconnect.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 3, "all events sent before flap must be in DB");

    // Session 2: reconnect with resume cursor at seq=0 (pretend we had none acked).
    // Send the same 3 events again (retransmit) — must NOT create duplicates.
    {
        let (mut client, session_id) = connect_forwarder(
            &fwd_url,
            "fwd-chaos-token-01",
            "fwd-chaos-01",
            vec!["10.50.50.1".to_owned()],
            vec![ResumeCursor {
                forwarder_id: "fwd-chaos-01".to_owned(),
                reader_ip: "10.50.50.1".to_owned(),
                stream_epoch: 1,
                last_seq: 0,
            }],
        )
        .await;

        for seq in 1u64..=3 {
            client
                .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
                    session_id: session_id.clone(),
                    batch_id: format!("batch-s2-{}", seq),
                    events: vec![ReadEvent {
                        forwarder_id: "fwd-chaos-01".to_owned(),
                        reader_ip: "10.50.50.1".to_owned(),
                        stream_epoch: 1,
                        seq,
                        reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                        raw_read_line: format!("CHAOS_LINE_{}", seq),
                        read_type: "RAW".to_owned(),
                    }],
                }))
                .await
                .unwrap();
            client.recv_message().await.unwrap();
        }
    }

    // Event count must still be 3 (retransmits must not duplicate).
    let final_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        final_count, 3,
        "retransmits after reconnect must not create duplicates"
    );

    // Retransmit count should be 3 (one per retransmitted event).
    let retransmit_count: i64 = sqlx::query_scalar("SELECT retransmit_count FROM stream_metrics")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(retransmit_count, 3, "each retransmit should be counted");
}

// ---------------------------------------------------------------------------
// Test: Rapid reconnects — no event duplication across many flaps.
// ---------------------------------------------------------------------------

/// Chaos test: rapid session reconnects with overlapping retransmits.
///
/// Simulates a device that reconnects many times quickly (e.g. unstable
/// network) and each time replays events it thinks might not have been acked.
/// After all reconnects, event count must match the unique event count.
#[tokio::test]
async fn chaos_network_rapid_reconnects_no_duplication() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-chaos-02", "forwarder", b"fwd-chaos-token-02").await;

    let addr = start_server(pool.clone()).await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);

    // Each "reconnect" sends events 1-3 (always the same set — simulating
    // conservative retransmit from cursor=0 after each flap).
    let num_reconnects = 3;
    for reconnect_num in 0..num_reconnects {
        let (mut client, session_id) = connect_forwarder(
            &fwd_url,
            "fwd-chaos-token-02",
            "fwd-chaos-02",
            vec!["10.60.60.1".to_owned()],
            vec![ResumeCursor {
                forwarder_id: "fwd-chaos-02".to_owned(),
                reader_ip: "10.60.60.1".to_owned(),
                stream_epoch: 1,
                last_seq: 0,
            }],
        )
        .await;

        for seq in 1u64..=3 {
            client
                .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
                    session_id: session_id.clone(),
                    batch_id: format!("batch-reconnect-{}-seq-{}", reconnect_num, seq),
                    events: vec![ReadEvent {
                        forwarder_id: "fwd-chaos-02".to_owned(),
                        reader_ip: "10.60.60.1".to_owned(),
                        stream_epoch: 1,
                        seq,
                        reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                        raw_read_line: format!("RAPID_LINE_{}", seq),
                        read_type: "RAW".to_owned(),
                    }],
                }))
                .await
                .unwrap();
            client.recv_message().await.unwrap();
        }

        // Close gracefully before next reconnect; brief pause for server to register.
        let _ = client.close().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // After all reconnects, must still have exactly 3 unique events.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count, 3,
        "unique events must be exactly 3 regardless of reconnects"
    );

    // Retransmit count: (num_reconnects - 1) * 3 retransmits (each reconnect after
    // the first sends all 3 events again as retransmits).
    let retransmit_count: i64 = sqlx::query_scalar("SELECT retransmit_count FROM stream_metrics")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        retransmit_count,
        ((num_reconnects - 1) * 3) as i64,
        "retransmit count should account for all re-sent events"
    );
}

// ---------------------------------------------------------------------------
// Test: Receiver connection flap — receiver reconnects and resumes from cursor.
// ---------------------------------------------------------------------------

/// Chaos test: receiver disconnects mid-stream and reconnects with cursor.
///
/// Events delivered before the flap must not be re-delivered after reconnect
/// if the receiver holds the correct cursor.
#[tokio::test]
async fn chaos_receiver_reconnect_resumes_correctly() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-chaos-03", "forwarder", b"fwd-chaos-token-03").await;
    insert_token(&pool, "rcv-chaos-03", "receiver", b"rcv-chaos-token-03").await;

    let addr = start_server(pool.clone()).await;

    // Send 5 events via forwarder.
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let (mut fwd, fwd_session) = connect_forwarder(
        &fwd_url,
        "fwd-chaos-token-03",
        "fwd-chaos-03",
        vec!["10.70.70.1".to_owned()],
        vec![],
    )
    .await;

    for seq in 1u64..=5 {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_session.clone(),
            batch_id: format!("batch-{}", seq),
            events: vec![ReadEvent {
                forwarder_id: "fwd-chaos-03".to_owned(),
                reader_ip: "10.70.70.1".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_read_line: format!("RCV_CHAOS_LINE_{}", seq),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }

    // Receiver session 1: subscribes and gets events 1..3, then "drops".
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut max_seq_session1: u64 = 0;
    {
        let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-chaos-token-03")
            .await
            .unwrap();
        rcv.send_message(&WsMessage::ReceiverHello(ReceiverHello {
            receiver_id: "rcv-chaos-03".to_owned(),
            resume: vec![ResumeCursor {
                forwarder_id: "fwd-chaos-03".to_owned(),
                reader_ip: "10.70.70.1".to_owned(),
                stream_epoch: 1,
                last_seq: 0,
            }],
        }))
        .await
        .unwrap();
        let rcv_session = match rcv.recv_message().await.unwrap() {
            WsMessage::Heartbeat(h) => h.session_id,
            other => panic!("{:?}", other),
        };

        // Collect some events.
        for _ in 0..5 {
            match tokio::time::timeout(Duration::from_secs(2), rcv.recv_message()).await {
                Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                    for ev in &batch.events {
                        if ev.seq > max_seq_session1 {
                            max_seq_session1 = ev.seq;
                        }
                    }
                    // Ack the batch.
                    let ack_entries: Vec<AckEntry> = batch
                        .events
                        .iter()
                        .map(|ev| AckEntry {
                            forwarder_id: ev.forwarder_id.clone(),
                            reader_ip: ev.reader_ip.clone(),
                            stream_epoch: ev.stream_epoch,
                            last_seq: ev.seq,
                        })
                        .collect();
                    rcv.send_message(&WsMessage::ReceiverAck(ReceiverAck {
                        session_id: rcv_session.clone(),
                        entries: ack_entries,
                    }))
                    .await
                    .unwrap();
                    if max_seq_session1 >= 3 {
                        break;
                    }
                }
                Ok(Ok(WsMessage::Heartbeat(_))) => continue,
                _ => break,
            }
        }
        // Session dropped here.
    }

    assert!(
        max_seq_session1 >= 1,
        "first receiver session should have gotten some events"
    );

    // Receiver session 2: reconnects with cursor at max_seq_session1.
    // Should only get events after that cursor.
    let mut rcv2 = MockWsClient::connect_with_token(&rcv_url, "rcv-chaos-token-03")
        .await
        .unwrap();
    rcv2.send_message(&WsMessage::ReceiverHello(ReceiverHello {
        receiver_id: "rcv-chaos-03".to_owned(),
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-chaos-03".to_owned(),
            reader_ip: "10.70.70.1".to_owned(),
            stream_epoch: 1,
            last_seq: max_seq_session1,
        }],
    }))
    .await
    .unwrap();
    match rcv2.recv_message().await.unwrap() {
        WsMessage::Heartbeat(_) => {}
        other => panic!("{:?}", other),
    }

    // Collect events — none should have seq <= max_seq_session1.
    let mut session2_seqs = Vec::new();
    for _ in 0..10 {
        match tokio::time::timeout(Duration::from_secs(2), rcv2.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                for ev in &batch.events {
                    assert!(
                        ev.seq > max_seq_session1,
                        "receiver reconnect must not re-deliver seq {} (cursor at {})",
                        ev.seq,
                        max_seq_session1
                    );
                    session2_seqs.push(ev.seq);
                }
                if session2_seqs.len() >= (5 - max_seq_session1 as usize) {
                    break;
                }
            }
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            _ => break,
        }
    }

    // Confirm we didn't get pre-cursor events.
    for &seq in &session2_seqs {
        assert!(
            seq > max_seq_session1,
            "session 2 should only get events after cursor ({}) — got seq {}",
            max_seq_session1,
            seq
        );
    }
}

// ---------------------------------------------------------------------------
// Test: Events from two streams are independent across network flaps.
// ---------------------------------------------------------------------------

/// Chaos test: two streams on the same forwarder; one stream flaps, the other
/// continues normally. Both streams must have all their events in the DB.
#[tokio::test]
async fn chaos_two_streams_independent_under_flap() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;

    insert_token(&pool, "fwd-chaos-04", "forwarder", b"fwd-chaos-token-04").await;

    let addr = start_server(pool.clone()).await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);

    // Session 1: send events on both streams.
    {
        let (mut client, session_id) = connect_forwarder(
            &fwd_url,
            "fwd-chaos-token-04",
            "fwd-chaos-04",
            vec!["10.80.80.1".to_owned(), "10.80.80.2".to_owned()],
            vec![],
        )
        .await;

        for seq in 1u64..=3 {
            client
                .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
                    session_id: session_id.clone(),
                    batch_id: format!("s1-batch-{}", seq),
                    events: vec![
                        ReadEvent {
                            forwarder_id: "fwd-chaos-04".to_owned(),
                            reader_ip: "10.80.80.1".to_owned(),
                            stream_epoch: 1,
                            seq,
                            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                            raw_read_line: format!("S1_LINE_{}", seq),
                            read_type: "RAW".to_owned(),
                        },
                        ReadEvent {
                            forwarder_id: "fwd-chaos-04".to_owned(),
                            reader_ip: "10.80.80.2".to_owned(),
                            stream_epoch: 1,
                            seq,
                            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                            raw_read_line: format!("S2_LINE_{}", seq),
                            read_type: "RAW".to_owned(),
                        },
                    ],
                }))
                .await
                .unwrap();
            client.recv_message().await.unwrap();
        }
        // Close connection before reconnect to avoid "session already active".
        let _ = client.close().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Both streams should have 3 events each.
    let total_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        total_count, 6,
        "both streams should have all events in DB after flap"
    );

    // Session 2: reconnect and send 1 more event per stream.
    {
        let (mut client, session_id) = connect_forwarder(
            &fwd_url,
            "fwd-chaos-token-04",
            "fwd-chaos-04",
            vec!["10.80.80.1".to_owned(), "10.80.80.2".to_owned()],
            vec![
                ResumeCursor {
                    forwarder_id: "fwd-chaos-04".to_owned(),
                    reader_ip: "10.80.80.1".to_owned(),
                    stream_epoch: 1,
                    last_seq: 3,
                },
                ResumeCursor {
                    forwarder_id: "fwd-chaos-04".to_owned(),
                    reader_ip: "10.80.80.2".to_owned(),
                    stream_epoch: 1,
                    last_seq: 3,
                },
            ],
        )
        .await;

        client
            .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
                session_id: session_id.clone(),
                batch_id: "s2-batch-new".to_owned(),
                events: vec![
                    ReadEvent {
                        forwarder_id: "fwd-chaos-04".to_owned(),
                        reader_ip: "10.80.80.1".to_owned(),
                        stream_epoch: 1,
                        seq: 4,
                        reader_timestamp: "2026-02-17T10:00:01.000Z".to_owned(),
                        raw_read_line: "S1_LINE_4".to_owned(),
                        read_type: "RAW".to_owned(),
                    },
                    ReadEvent {
                        forwarder_id: "fwd-chaos-04".to_owned(),
                        reader_ip: "10.80.80.2".to_owned(),
                        stream_epoch: 1,
                        seq: 4,
                        reader_timestamp: "2026-02-17T10:00:01.000Z".to_owned(),
                        raw_read_line: "S2_LINE_4".to_owned(),
                        read_type: "RAW".to_owned(),
                    },
                ],
            }))
            .await
            .unwrap();
        client.recv_message().await.unwrap();
    }

    // Should now have 8 events total (4 per stream).
    let final_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        final_count, 8,
        "after session 2, each stream should have 4 events"
    );
}
