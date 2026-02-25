//! Integration tests for receiver WS cursor-based resume from DB.
use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, time::Duration};
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

async fn receiver_handshake(
    client: &mut MockWsClient,
    receiver_id: &str,
    resume: Vec<ResumeCursor>,
) -> String {
    let mut seen = HashSet::new();
    let streams: Vec<StreamRef> = resume
        .iter()
        .filter_map(|cursor| {
            let key = (cursor.forwarder_id.clone(), cursor.reader_ip.clone());
            if seen.insert(key.clone()) {
                Some(StreamRef {
                    forwarder_id: key.0,
                    reader_ip: key.1,
                })
            } else {
                None
            }
        })
        .collect();

    client
        .send_message(&WsMessage::ReceiverHelloV12(ReceiverHelloV12 {
            receiver_id: receiver_id.to_owned(),
            mode: ReceiverMode::Live {
                streams,
                earliest_epochs: vec![],
            },
            resume,
        }))
        .await
        .unwrap();

    loop {
        match client.recv_message().await.unwrap() {
            WsMessage::Heartbeat(hb) => break hb.session_id,
            WsMessage::ReceiverModeApplied(_) => {}
            other => panic!("expected Heartbeat or ReceiverModeApplied, got {:?}", other),
        }
    }
}

/// Test that a receiver resuming from cursor only gets events after the cursor.
#[tokio::test]
async fn test_receiver_resume_from_cursor() {
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

    insert_token(&pool, "fwd-resume", "forwarder", b"fwd-resume-token").await;
    insert_token(&pool, "rcv-resume", "receiver", b"rcv-resume-token").await;

    // Connect forwarder and send 3 events
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-resume-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-resume".to_owned(),
        reader_ips: vec!["10.2.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    for seq in 1..=3u64 {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_session.clone(),
            batch_id: format!("b{}", seq),
            events: vec![ReadEvent {
                forwarder_id: "fwd-resume".to_owned(),
                reader_ip: "10.2.0.1:10000".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_frame: format!("LINE_{}", seq).into_bytes(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }

    let stream_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2",
    )
    .bind("fwd-resume")
    .bind("10.2.0.1:10000")
    .fetch_one(&pool)
    .await
    .unwrap();
    server::repo::receiver_cursors::upsert_cursor(&pool, "rcv-resume", stream_id, 1, 2)
        .await
        .unwrap();

    // Receiver connects; persisted cursor at seq=2 means replay starts at 3.
    let rcv_url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-resume-token")
        .await
        .unwrap();
    let _rcv_session = receiver_handshake(
        &mut rcv,
        "rcv-resume",
        vec![ResumeCursor {
            forwarder_id: "fwd-resume".to_owned(),
            reader_ip: "10.2.0.1:10000".to_owned(),
            stream_epoch: 1,
            last_seq: 0,
        }],
    )
    .await;

    // Should receive only seq=3 as replay (not 1 or 2)
    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            assert_eq!(batch.events.len(), 1, "should replay only 1 event (seq=3)");
            assert_eq!(batch.events[0].seq, 3);
            assert_eq!(batch.events[0].raw_frame, b"LINE_3".to_vec());
        }
        Ok(Ok(other)) => panic!("expected ReceiverEventBatch, got {:?}", other),
        Ok(Err(e)) => panic!("{}", e),
        Err(_) => panic!("timeout waiting for replay events"),
    }
}

#[tokio::test]
async fn test_receiver_v12_prefers_persisted_cursor_over_hello_resume() {
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

    insert_token(
        &pool,
        "fwd-legacy-resume-cursor",
        "forwarder",
        b"fwd-legacy-resume-cursor-token",
    )
    .await;
    insert_token(
        &pool,
        "rcv-legacy-resume-cursor",
        "receiver",
        b"rcv-legacy-resume-cursor-token",
    )
    .await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-legacy-resume-cursor-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-legacy-resume-cursor".to_owned(),
        reader_ips: vec!["10.22.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    for seq in 1..=3u64 {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_session.clone(),
            batch_id: format!("legacy-resume-{}", seq),
            events: vec![ReadEvent {
                forwarder_id: "fwd-legacy-resume-cursor".to_owned(),
                reader_ip: "10.22.0.1:10000".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_frame: format!("LEGACY_RESUME_{}", seq).into_bytes(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }

    let stream_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2",
    )
    .bind("fwd-legacy-resume-cursor")
    .bind("10.22.0.1:10000")
    .fetch_one(&pool)
    .await
    .unwrap();

    server::repo::receiver_cursors::upsert_cursor(
        &pool,
        "rcv-legacy-resume-cursor",
        stream_id,
        1,
        3,
    )
    .await
    .unwrap();

    let rcv_url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-legacy-resume-cursor-token")
        .await
        .unwrap();
    let _rcv_session = receiver_handshake(
        &mut rcv,
        "rcv-legacy-resume-cursor",
        vec![ResumeCursor {
            forwarder_id: "fwd-legacy-resume-cursor".to_owned(),
            reader_ip: "10.22.0.1:10000".to_owned(),
            stream_epoch: 1,
            last_seq: 1,
        }],
    )
    .await;

    match tokio::time::timeout(Duration::from_secs(1), rcv.recv_message()).await {
        Err(_) => {}
        Ok(Ok(WsMessage::Heartbeat(_) | WsMessage::ReceiverModeApplied(_))) => {}
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            panic!(
                "expected no replay with persisted cursor at tail, got {:?}",
                batch
            )
        }
        Ok(Ok(other)) => panic!("unexpected message {:?}", other),
        Ok(Err(e)) => panic!("{}", e),
    }
}

/// Test that a receiver with no resume cursor gets all events.
#[tokio::test]
async fn test_receiver_no_cursor_gets_all() {
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

    insert_token(&pool, "fwd-all", "forwarder", b"fwd-all-token").await;
    insert_token(&pool, "rcv-all", "receiver", b"rcv-all-token").await;

    // Connect forwarder and send 2 events
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-all-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-all".to_owned(),
        reader_ips: vec!["10.3.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    for seq in 1..=2u64 {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_session.clone(),
            batch_id: format!("b{}", seq),
            events: vec![ReadEvent {
                forwarder_id: "fwd-all".to_owned(),
                reader_ip: "10.3.0.1:10000".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_frame: format!("ALL_LINE_{}", seq).into_bytes(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }

    // Receiver with no resume cursor - subscribes to the stream
    let rcv_url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-all-token")
        .await
        .unwrap();
    let _rcv_session = receiver_handshake(
        &mut rcv,
        "rcv-all",
        vec![ResumeCursor {
            forwarder_id: "fwd-all".to_owned(),
            reader_ip: "10.3.0.1:10000".to_owned(),
            stream_epoch: 1,
            last_seq: 0,
        }],
    )
    .await;

    // Should receive both events as replay
    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            assert!(
                !batch.events.is_empty(),
                "should get at least 1 replayed event"
            );
            // Events should start from seq=1 since cursor was at 0
            assert_eq!(batch.events[0].seq, 1);
        }
        Ok(Ok(other)) => panic!("expected ReceiverEventBatch, got {:?}", other),
        Ok(Err(e)) => panic!("{}", e),
        Err(_) => panic!("timeout waiting for replay events"),
    }
}

/// Test that large backlog replay is chunked into multiple receiver batches.
#[tokio::test]
async fn test_receiver_large_backlog_replay_is_chunked() {
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

    insert_token(&pool, "fwd-chunk", "forwarder", b"fwd-chunk-token").await;
    insert_token(&pool, "rcv-chunk", "receiver", b"rcv-chunk-token").await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-chunk-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-chunk".to_owned(),
        reader_ips: vec!["10.4.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    let events: Vec<ReadEvent> = (1..=600u64)
        .map(|seq| ReadEvent {
            forwarder_id: "fwd-chunk".to_owned(),
            reader_ip: "10.4.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_frame: format!("CHUNK_LINE_{}", seq).into_bytes(),
            read_type: "RAW".to_owned(),
        })
        .collect();

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "large-batch".to_owned(),
        events,
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let rcv_url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-chunk-token")
        .await
        .unwrap();
    let _rcv_session = receiver_handshake(
        &mut rcv,
        "rcv-chunk",
        vec![ResumeCursor {
            forwarder_id: "fwd-chunk".to_owned(),
            reader_ip: "10.4.0.1:10000".to_owned(),
            stream_epoch: 1,
            last_seq: 0,
        }],
    )
    .await;

    let mut batch_count = 0usize;
    let mut seqs: Vec<u64> = Vec::new();
    while seqs.len() < 600 {
        match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                batch_count += 1;
                seqs.extend(batch.events.iter().map(|e| e.seq));
            }
            Ok(Ok(other)) => panic!("expected ReceiverEventBatch, got {:?}", other),
            Ok(Err(e)) => panic!("{}", e),
            Err(_) => panic!("timeout waiting for chunked replay"),
        }
    }

    assert!(
        batch_count >= 2,
        "expected replay to be split into multiple batches"
    );
    assert_eq!(seqs.len(), 600);
    assert_eq!(seqs.first().copied(), Some(1));
    assert_eq!(seqs.last().copied(), Some(600));
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
}

/// If cursor is already at tail, receiver should not get replayed events.
#[tokio::test]
async fn test_receiver_tail_at_cursor_gets_no_replay() {
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

    insert_token(&pool, "fwd-tail", "forwarder", b"fwd-tail-token").await;
    insert_token(&pool, "rcv-tail", "receiver", b"rcv-tail-token").await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-tail-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-tail".to_owned(),
        reader_ips: vec!["10.5.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    let events: Vec<ReadEvent> = (1..=5u64)
        .map(|seq| ReadEvent {
            forwarder_id: "fwd-tail".to_owned(),
            reader_ip: "10.5.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_frame: format!("TAIL_LINE_{}", seq).into_bytes(),
            read_type: "RAW".to_owned(),
        })
        .collect();
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "tail-batch".to_owned(),
        events,
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let stream_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2",
    )
    .bind("fwd-tail")
    .bind("10.5.0.1:10000")
    .fetch_one(&pool)
    .await
    .unwrap();
    server::repo::receiver_cursors::upsert_cursor(&pool, "rcv-tail", stream_id, 1, 5)
        .await
        .unwrap();

    let rcv_url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-tail-token")
        .await
        .unwrap();
    let _rcv_session = receiver_handshake(
        &mut rcv,
        "rcv-tail",
        vec![ResumeCursor {
            forwarder_id: "fwd-tail".to_owned(),
            reader_ip: "10.5.0.1:10000".to_owned(),
            stream_epoch: 1,
            last_seq: 0,
        }],
    )
    .await;

    match tokio::time::timeout(Duration::from_secs(1), rcv.recv_message()).await {
        Err(_) => {} // expected: no replay batch
        Ok(Ok(WsMessage::Heartbeat(_) | WsMessage::ReceiverModeApplied(_))) => {} // heartbeat is fine
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            panic!("expected no replay, got {:?}", batch)
        }
        Ok(Ok(other)) => panic!("unexpected message {:?}", other),
        Ok(Err(e)) => panic!("{}", e),
    }
}

/// Lower bound is exclusive: cursor at seq N should replay from N+1.
#[tokio::test]
async fn test_receiver_replay_lower_bound_is_exclusive() {
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

    insert_token(&pool, "fwd-lower", "forwarder", b"fwd-lower-token").await;
    insert_token(&pool, "rcv-lower", "receiver", b"rcv-lower-token").await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-lower-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-lower".to_owned(),
        reader_ips: vec!["10.6.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    let events: Vec<ReadEvent> = (1..=4u64)
        .map(|seq| ReadEvent {
            forwarder_id: "fwd-lower".to_owned(),
            reader_ip: "10.6.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_frame: format!("LOWER_LINE_{}", seq).into_bytes(),
            read_type: "RAW".to_owned(),
        })
        .collect();
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "lower-batch".to_owned(),
        events,
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let stream_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2",
    )
    .bind("fwd-lower")
    .bind("10.6.0.1:10000")
    .fetch_one(&pool)
    .await
    .unwrap();
    server::repo::receiver_cursors::upsert_cursor(&pool, "rcv-lower", stream_id, 1, 2)
        .await
        .unwrap();

    let rcv_url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-lower-token")
        .await
        .unwrap();
    let _rcv_session = receiver_handshake(
        &mut rcv,
        "rcv-lower",
        vec![ResumeCursor {
            forwarder_id: "fwd-lower".to_owned(),
            reader_ip: "10.6.0.1:10000".to_owned(),
            stream_epoch: 1,
            last_seq: 0,
        }],
    )
    .await;

    let first_replayed_seq =
        match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => batch.events[0].seq,
            Ok(Ok(other)) => panic!("expected replay batch, got {:?}", other),
            Ok(Err(e)) => panic!("{}", e),
            Err(_) => panic!("timeout waiting for replay batch"),
        };
    assert_eq!(first_replayed_seq, 3);
}

/// Replay/live handoff should preserve monotonic ordering.
#[tokio::test]
async fn test_receiver_handoff_remains_monotonic() {
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

    insert_token(&pool, "fwd-mono", "forwarder", b"fwd-mono-token").await;
    insert_token(&pool, "rcv-mono", "receiver", b"rcv-mono-token").await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-mono-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-mono".to_owned(),
        reader_ips: vec!["10.7.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    let backlog: Vec<ReadEvent> = (1..=20u64)
        .map(|seq| ReadEvent {
            forwarder_id: "fwd-mono".to_owned(),
            reader_ip: "10.7.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_frame: format!("MONO_BACKLOG_{}", seq).into_bytes(),
            read_type: "RAW".to_owned(),
        })
        .collect();
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session.clone(),
        batch_id: "mono-backlog".to_owned(),
        events: backlog,
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let rcv_url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-mono-token")
        .await
        .unwrap();
    let _rcv_session = receiver_handshake(
        &mut rcv,
        "rcv-mono",
        vec![ResumeCursor {
            forwarder_id: "fwd-mono".to_owned(),
            reader_ip: "10.7.0.1:10000".to_owned(),
            stream_epoch: 1,
            last_seq: 0,
        }],
    )
    .await;

    let live: Vec<ReadEvent> = (21..=30u64)
        .map(|seq| ReadEvent {
            forwarder_id: "fwd-mono".to_owned(),
            reader_ip: "10.7.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_frame: format!("MONO_LIVE_{}", seq).into_bytes(),
            read_type: "RAW".to_owned(),
        })
        .collect();
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "mono-live".to_owned(),
        events: live,
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let mut seqs: Vec<u64> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline && seqs.len() < 30 {
        match tokio::time::timeout(Duration::from_secs(1), rcv.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                seqs.extend(batch.events.into_iter().map(|e| e.seq));
            }
            Ok(Ok(WsMessage::Heartbeat(_) | WsMessage::ReceiverModeApplied(_))) => {}
            Ok(Ok(other)) => panic!("unexpected message {:?}", other),
            Ok(Err(e)) => panic!("{}", e),
            Err(_) => {}
        }
    }

    assert!(
        seqs.len() >= 30,
        "expected at least 30 events, got {}",
        seqs.len()
    );
    assert!(seqs.windows(2).all(|w| w[0] < w[1]));
}

/// Under very heavy replay load, live mode with multiple streams should still
/// deliver both streams promptly.
#[tokio::test]
async fn test_receiver_live_mode_progresses_under_heavy_replay() {
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

    insert_token(&pool, "fwd-main", "forwarder", b"fwd-main-token").await;
    insert_token(&pool, "fwd-side", "forwarder", b"fwd-side-token").await;
    insert_token(&pool, "rcv-heavy", "receiver", b"rcv-heavy-token").await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd_main = MockWsClient::connect_with_token(&fwd_url, "fwd-main-token")
        .await
        .unwrap();
    fwd_main
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-main".to_owned(),
            reader_ips: vec!["10.8.0.1:10000".to_owned()],
            display_name: None,
        }))
        .await
        .unwrap();
    let fwd_main_session = match fwd_main.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    let mut fwd_side = MockWsClient::connect_with_token(&fwd_url, "fwd-side-token")
        .await
        .unwrap();
    fwd_side
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-side".to_owned(),
            reader_ips: vec!["10.8.0.2:10000".to_owned()],
            display_name: None,
        }))
        .await
        .unwrap();
    let fwd_side_session = match fwd_side.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    let heavy_backlog: Vec<ReadEvent> = (1..=10_000u64)
        .map(|seq| ReadEvent {
            forwarder_id: "fwd-main".to_owned(),
            reader_ip: "10.8.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_frame: format!("MAIN_HEAVY_{}", seq).into_bytes(),
            read_type: "RAW".to_owned(),
        })
        .collect();
    fwd_main
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_main_session.clone(),
            batch_id: "heavy-main".to_owned(),
            events: heavy_backlog,
        }))
        .await
        .unwrap();
    fwd_main.recv_message().await.unwrap();

    let side_events: Vec<ReadEvent> = (1..=3u64)
        .map(|seq| ReadEvent {
            forwarder_id: "fwd-side".to_owned(),
            reader_ip: "10.8.0.2:10000".to_owned(),
            stream_epoch: 1,
            seq,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_frame: format!("SIDE_{}", seq).into_bytes(),
            read_type: "RAW".to_owned(),
        })
        .collect();
    fwd_side
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_side_session,
            batch_id: "side-batch".to_owned(),
            events: side_events,
        }))
        .await
        .unwrap();
    fwd_side.recv_message().await.unwrap();

    let rcv_url = format!("ws://{}/ws/v1.2/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-heavy-token")
        .await
        .unwrap();
    let _rcv_session = receiver_handshake(
        &mut rcv,
        "rcv-heavy",
        vec![
            ResumeCursor {
                forwarder_id: "fwd-side".to_owned(),
                reader_ip: "10.8.0.2:10000".to_owned(),
                stream_epoch: 1,
                last_seq: 0,
            },
            ResumeCursor {
                forwarder_id: "fwd-main".to_owned(),
                reader_ip: "10.8.0.1:10000".to_owned(),
                stream_epoch: 1,
                last_seq: 0,
            },
        ],
    )
    .await;

    let main_stream_id = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2",
    )
    .bind("fwd-main")
    .bind("10.8.0.1:10000")
    .fetch_one(&pool)
    .await
    .unwrap();

    let pool_for_inserts = pool.clone();
    let ingest_task = tokio::spawn(async move {
        for seq in 10_001_i64..=120_000_i64 {
            let _ = server::repo::events::upsert_event(
                &pool_for_inserts,
                main_stream_id,
                1,
                seq,
                "2026-02-17T10:00:00.000Z",
                format!("MAIN_DB_{}", seq).as_bytes(),
                "RAW",
            )
            .await;
        }
    });

    let live_started_at = tokio::time::Instant::now();
    let deadline = live_started_at + Duration::from_secs(5);
    let mut saw_side_stream = false;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), rcv.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                for event in &batch.events {
                    if event.reader_ip == "10.8.0.2:10000" {
                        saw_side_stream = true;
                        break;
                    }
                }
                if saw_side_stream {
                    break;
                }
            }
            Ok(Ok(WsMessage::Heartbeat(_) | WsMessage::ReceiverModeApplied(_))) => {}
            Ok(Ok(_)) => {}
            Ok(Err(e)) => panic!("{}", e),
            Err(_) => {}
        }
    }

    assert!(
        saw_side_stream,
        "expected side stream event within 5s during heavy replay; elapsed={:?}",
        live_started_at.elapsed()
    );
    ingest_task.abort();
}
