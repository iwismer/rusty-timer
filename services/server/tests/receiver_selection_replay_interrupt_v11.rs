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

async fn start_server() -> (sqlx::PgPool, std::net::SocketAddr) {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    std::mem::forget(container);
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
    (pool, addr)
}

async fn connect_forwarder(
    pool: &sqlx::PgPool,
    addr: std::net::SocketAddr,
) -> (MockWsClient, String) {
    insert_token(pool, "fwd-ri", "forwarder", b"fwd-ri-token").await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&url, "fwd-ri-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-ri".to_owned(),
        reader_ips: vec!["10.40.0.1:10000".to_owned(), "10.40.0.2:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let session_id = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {:?}", other),
    };
    (fwd, session_id)
}

#[tokio::test]
async fn live_only_policy_skips_backfill_on_connect() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-ri", "receiver", b"rcv-ri-token").await;

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session.clone(),
        batch_id: "pre".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-ri".to_owned(),
            reader_ip: "10.40.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-23T10:00:00.000Z".to_owned(),
            raw_read_line: "BACKFILL".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-ri-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-ri".to_owned(),
        selection: ReceiverSelection::Manual {
            streams: vec![StreamRef {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.1:10000".to_owned(),
            }],
        },
        replay_policy: ReplayPolicy::LiveOnly,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    let _ = rcv.recv_message().await.unwrap();

    match tokio::time::timeout(Duration::from_millis(400), rcv.recv_message()).await {
        Err(_) => {}
        Ok(Ok(other)) => panic!("expected no backfill replay, got {:?}", other),
        Ok(Err(_)) => {}
    }

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "live".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-ri".to_owned(),
            reader_ip: "10.40.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 2,
            reader_timestamp: "2026-02-23T10:00:01.000Z".to_owned(),
            raw_read_line: "LIVE_ONLY".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            assert_eq!(batch.events.len(), 1);
            assert_eq!(batch.events[0].raw_read_line, "LIVE_ONLY");
        }
        Ok(Ok(other)) => panic!("expected receiver_event_batch, got {:?}", other),
        Ok(Err(e)) => panic!("recv error: {}", e),
        Err(_) => panic!("timeout waiting for live event"),
    }
}

#[tokio::test]
async fn targeted_policy_replays_only_explicit_stream_epochs() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-ri", "receiver", b"rcv-ri-token").await;

    for (batch_id, event) in [
        (
            "b1",
            ReadEvent {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.1:10000".to_owned(),
                stream_epoch: 1,
                seq: 1,
                reader_timestamp: "2026-02-23T10:00:00.000Z".to_owned(),
                raw_read_line: "R1E1S1".to_owned(),
                read_type: "RAW".to_owned(),
            },
        ),
        (
            "b2",
            ReadEvent {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.1:10000".to_owned(),
                stream_epoch: 1,
                seq: 2,
                reader_timestamp: "2026-02-23T10:00:01.000Z".to_owned(),
                raw_read_line: "R1E1S2".to_owned(),
                read_type: "RAW".to_owned(),
            },
        ),
        (
            "b3",
            ReadEvent {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.1:10000".to_owned(),
                stream_epoch: 2,
                seq: 1,
                reader_timestamp: "2026-02-23T10:00:02.000Z".to_owned(),
                raw_read_line: "R1E2S1".to_owned(),
                read_type: "RAW".to_owned(),
            },
        ),
        (
            "b4",
            ReadEvent {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.2:10000".to_owned(),
                stream_epoch: 1,
                seq: 1,
                reader_timestamp: "2026-02-23T10:00:03.000Z".to_owned(),
                raw_read_line: "R2E1S1".to_owned(),
                read_type: "RAW".to_owned(),
            },
        ),
    ] {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_session.clone(),
            batch_id: batch_id.to_owned(),
            events: vec![event],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-ri-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-ri".to_owned(),
        selection: ReceiverSelection::Manual {
            streams: vec![
                StreamRef {
                    forwarder_id: "fwd-ri".to_owned(),
                    reader_ip: "10.40.0.1:10000".to_owned(),
                },
                StreamRef {
                    forwarder_id: "fwd-ri".to_owned(),
                    reader_ip: "10.40.0.2:10000".to_owned(),
                },
            ],
        },
        replay_policy: ReplayPolicy::Targeted,
        replay_targets: Some(vec![ReplayTarget {
            forwarder_id: "fwd-ri".to_owned(),
            reader_ip: "10.40.0.1:10000".to_owned(),
            stream_epoch: 1,
            from_seq: 2,
        }]),
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    let msg1 = rcv.recv_message().await.unwrap();
    let msg2 = rcv.recv_message().await.unwrap();
    let replay_batch = match (msg1, msg2) {
        (WsMessage::ReceiverEventBatch(batch), WsMessage::ReceiverSelectionApplied(_)) => batch,
        (WsMessage::ReceiverSelectionApplied(_), WsMessage::ReceiverEventBatch(batch)) => batch,
        (a, b) => panic!(
            "expected selection_applied + targeted replay, got {:?} / {:?}",
            a, b
        ),
    };
    assert_eq!(replay_batch.events.len(), 1);
    let event = &replay_batch.events[0];
    assert_eq!(event.reader_ip, "10.40.0.1:10000");
    assert_eq!(event.stream_epoch, 1);
    assert_eq!(event.seq, 2);
    assert_eq!(event.raw_read_line, "R1E1S2");

    match tokio::time::timeout(Duration::from_millis(400), rcv.recv_message()).await {
        Err(_) => {}
        Ok(Ok(other)) => panic!("unexpected extra replay batch: {:?}", other),
        Ok(Err(_)) => {}
    }
}

#[tokio::test]
async fn targeted_policy_replay_is_snapshot_bounded() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-ri", "receiver", b"rcv-ri-token").await;

    for seq in 1_u64..=1000_u64 {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_session.clone(),
            batch_id: format!("seed-{seq}"),
            events: vec![ReadEvent {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.1:10000".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-23T10:00:00.000Z".to_owned(),
                raw_read_line: format!("R1E1S{seq}"),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }
    let stream_id: sqlx::types::Uuid = sqlx::query_scalar(
        "SELECT stream_id FROM streams WHERE forwarder_id = 'fwd-ri' AND reader_ip = '10.40.0.1:10000'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-ri-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-ri".to_owned(),
        selection: ReceiverSelection::Manual {
            streams: vec![StreamRef {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.1:10000".to_owned(),
            }],
        },
        replay_policy: ReplayPolicy::Targeted,
        replay_targets: Some(vec![ReplayTarget {
            forwarder_id: "fwd-ri".to_owned(),
            reader_ip: "10.40.0.1:10000".to_owned(),
            stream_epoch: 1,
            from_seq: 1,
        }]),
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    let first_msg = rcv.recv_message().await.unwrap();
    let first_replay = match first_msg {
        WsMessage::ReceiverEventBatch(batch) => batch,
        other => panic!("expected first replay batch, got {:?}", other),
    };
    assert_eq!(first_replay.events.len(), 500);
    assert_eq!(first_replay.events.first().map(|e| e.seq), Some(1));
    assert_eq!(first_replay.events.last().map(|e| e.seq), Some(500));

    sqlx::query(
        r#"INSERT INTO events (stream_id, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, tag_id)
           SELECT $1, 1, gs, '2026-02-23T10:00:01.000Z', CONCAT('R1E1S', gs::text), 'RAW', NULL
           FROM generate_series(1001, 1600) AS gs"#,
    )
    .bind(stream_id)
    .execute(&pool)
    .await
    .unwrap();

    let mut replay_seqs: Vec<u64> = first_replay.events.iter().map(|event| event.seq).collect();
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(5), rcv.recv_message())
            .await
            .expect("timeout waiting for replay/selection message")
            .expect("receiver closed unexpectedly");
        match msg {
            WsMessage::ReceiverEventBatch(batch) => {
                replay_seqs.extend(batch.events.into_iter().map(|event| event.seq));
            }
            WsMessage::ReceiverSelectionApplied(_) => break,
            other => panic!("unexpected message before selection applied: {:?}", other),
        }
    }

    assert_eq!(replay_seqs.len(), 1000);
    assert!(!replay_seqs.contains(&1001));
    assert_eq!(replay_seqs.first().copied(), Some(1));
    assert_eq!(replay_seqs.last().copied(), Some(1000));
}

#[tokio::test]
async fn set_selection_interrupts_long_targeted_replay_at_batch_boundary() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-ri", "receiver", b"rcv-ri-token").await;

    let stream_one_id: sqlx::types::Uuid = sqlx::query_scalar(
        "SELECT stream_id FROM streams WHERE forwarder_id = 'fwd-ri' AND reader_ip = '10.40.0.1:10000'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT INTO events (stream_id, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, tag_id)
           SELECT $1, 1, gs, '2026-02-23T10:00:00.000Z', CONCAT('R1E1S', gs::text), 'RAW', NULL
           FROM generate_series(1, 200000) AS gs"#,
    )
    .bind(stream_one_id)
    .execute(&pool)
    .await
    .unwrap();

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "stream-two".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-ri".to_owned(),
            reader_ip: "10.40.0.2:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-23T10:00:00.000Z".to_owned(),
            raw_read_line: "R2E1S1".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-ri-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-ri".to_owned(),
        selection: ReceiverSelection::Manual {
            streams: vec![StreamRef {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.1:10000".to_owned(),
            }],
        },
        replay_policy: ReplayPolicy::Targeted,
        replay_targets: Some(vec![ReplayTarget {
            forwarder_id: "fwd-ri".to_owned(),
            reader_ip: "10.40.0.1:10000".to_owned(),
            stream_epoch: 1,
            from_seq: 1,
        }]),
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    let first_batch = match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => batch,
        Ok(Ok(other)) => panic!("expected first replay batch, got {:?}", other),
        Ok(Err(e)) => panic!("recv error: {}", e),
        Err(_) => panic!("timeout waiting for first replay batch"),
    };
    assert_eq!(
        first_batch
            .events
            .first()
            .map(|event| event.reader_ip.as_str()),
        Some("10.40.0.1:10000")
    );

    rcv.send_message(&WsMessage::ReceiverSetSelection(ReceiverSetSelection {
        selection: ReceiverSelection::Manual {
            streams: vec![StreamRef {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.2:10000".to_owned(),
            }],
        },
        replay_policy: ReplayPolicy::Targeted,
        replay_targets: Some(vec![ReplayTarget {
            forwarder_id: "fwd-ri".to_owned(),
            reader_ip: "10.40.0.2:10000".to_owned(),
            stream_epoch: 1,
            from_seq: 1,
        }]),
    }))
    .await
    .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    let mut saw_selection_applied = false;
    let mut saw_stream_two_replay = false;
    while tokio::time::Instant::now() < deadline {
        let msg = match tokio::time::timeout(Duration::from_millis(200), rcv.recv_message()).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => panic!("recv error: {}", e),
            Err(_) => continue,
        };
        match msg {
            WsMessage::ReceiverSelectionApplied(applied) => {
                assert_eq!(applied.resolved_target_count, 1);
                match applied.selection {
                    ReceiverSelection::Manual { streams } => {
                        assert_eq!(streams.len(), 1);
                        assert_eq!(streams[0].reader_ip, "10.40.0.2:10000");
                    }
                    other => panic!("expected manual selection, got {:?}", other),
                }
                saw_selection_applied = true;
            }
            WsMessage::ReceiverEventBatch(batch) => {
                if batch
                    .events
                    .iter()
                    .any(|event| event.reader_ip == "10.40.0.2:10000" && event.seq == 1)
                {
                    saw_stream_two_replay = true;
                }
            }
            WsMessage::Heartbeat(_) => {}
            other => panic!("unexpected message: {:?}", other),
        }
        if saw_selection_applied && saw_stream_two_replay {
            break;
        }
    }

    assert!(
        saw_selection_applied,
        "expected new selection_applied within 2s"
    );
    assert!(
        saw_stream_two_replay,
        "expected replay from the new selection within 2s"
    );
}

#[tokio::test]
async fn set_selection_replay_restart_uses_persisted_cursor_for_new_selection() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-ri", "receiver", b"rcv-ri-token").await;

    let stream_one_id: sqlx::types::Uuid = sqlx::query_scalar(
        "SELECT stream_id FROM streams WHERE forwarder_id = 'fwd-ri' AND reader_ip = '10.40.0.1:10000'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let stream_two_id: sqlx::types::Uuid = sqlx::query_scalar(
        "SELECT stream_id FROM streams WHERE forwarder_id = 'fwd-ri' AND reader_ip = '10.40.0.2:10000'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT INTO events (stream_id, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, tag_id)
           SELECT $1, 1, gs, '2026-02-23T10:00:00.000Z', CONCAT('R1E1S', gs::text), 'RAW', NULL
           FROM generate_series(1, 200000) AS gs"#,
    )
    .bind(stream_one_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT INTO events (stream_id, stream_epoch, seq, reader_timestamp, raw_read_line, read_type, tag_id)
           VALUES
           ($1, 1, 1, '2026-02-23T10:00:00.000Z', 'R2E1S1', 'RAW', NULL),
           ($1, 1, 2, '2026-02-23T10:00:01.000Z', 'R2E1S2', 'RAW', NULL),
           ($1, 1, 3, '2026-02-23T10:00:02.000Z', 'R2E1S3', 'RAW', NULL)"#,
    )
    .bind(stream_two_id)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT INTO receiver_cursors (receiver_id, stream_id, stream_epoch, last_seq)
           VALUES ('rcv-ri', $1, 1, 1)
           ON CONFLICT (receiver_id, stream_id, stream_epoch)
           DO UPDATE SET last_seq = 1"#,
    )
    .bind(stream_two_id)
    .execute(&pool)
    .await
    .unwrap();

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "stream-two-live".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-ri".to_owned(),
            reader_ip: "10.40.0.2:10000".to_owned(),
            stream_epoch: 1,
            seq: 4,
            reader_timestamp: "2026-02-23T10:00:03.000Z".to_owned(),
            raw_read_line: "R2E1S4".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-ri-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-ri".to_owned(),
        selection: ReceiverSelection::Manual {
            streams: vec![StreamRef {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.1:10000".to_owned(),
            }],
        },
        replay_policy: ReplayPolicy::Targeted,
        replay_targets: Some(vec![ReplayTarget {
            forwarder_id: "fwd-ri".to_owned(),
            reader_ip: "10.40.0.1:10000".to_owned(),
            stream_epoch: 1,
            from_seq: 1,
        }]),
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), rcv.recv_message())
        .await
        .expect("timeout waiting for first replay batch")
        .expect("receiver closed unexpectedly");

    rcv.send_message(&WsMessage::ReceiverSetSelection(ReceiverSetSelection {
        selection: ReceiverSelection::Manual {
            streams: vec![StreamRef {
                forwarder_id: "fwd-ri".to_owned(),
                reader_ip: "10.40.0.2:10000".to_owned(),
            }],
        },
        replay_policy: ReplayPolicy::Resume,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut saw_selection_applied = false;
    let mut resumed_seqs: Vec<u64> = Vec::new();
    while tokio::time::Instant::now() < deadline {
        let msg = match tokio::time::timeout(Duration::from_millis(250), rcv.recv_message()).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => panic!("recv error: {}", e),
            Err(_) => continue,
        };
        match msg {
            WsMessage::ReceiverSelectionApplied(applied) => {
                assert_eq!(applied.replay_policy, ReplayPolicy::Resume);
                saw_selection_applied = true;
            }
            WsMessage::ReceiverEventBatch(batch) => {
                resumed_seqs.extend(
                    batch
                        .events
                        .into_iter()
                        .filter(|event| event.reader_ip == "10.40.0.2:10000")
                        .map(|event| event.seq),
                );
            }
            WsMessage::Heartbeat(_) => {}
            other => panic!("unexpected message: {:?}", other),
        }
        if saw_selection_applied && resumed_seqs.contains(&2) && resumed_seqs.contains(&3) {
            break;
        }
    }

    assert!(
        saw_selection_applied,
        "expected selection_applied for replacement selection"
    );
    assert!(
        resumed_seqs.contains(&2),
        "expected replay for new selection to include seq=2 from persisted cursor"
    );
    assert!(
        resumed_seqs.contains(&3),
        "expected replay for new selection to include seq=3 from persisted cursor"
    );
    assert!(
        !resumed_seqs.contains(&1),
        "did not expect replay to include seq=1 that is already persisted"
    );
}
