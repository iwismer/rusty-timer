//! Integration tests for receiver WS cursor-based resume from DB.
use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use std::time::Duration;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

async fn insert_token(pool: &sqlx::PgPool, device_id: &str, device_type: &str, raw_token: &[u8]) {
    let hash = Sha256::digest(raw_token);
    sqlx::query!("INSERT INTO device_tokens (token_hash, device_type, device_id) VALUES ($1, $2, $3)", hash.as_slice(), device_type, device_id)
        .execute(pool).await.unwrap();
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
    tokio::spawn(async move { axum::serve(listener, server::build_router(app_state)).await.unwrap(); });

    insert_token(&pool, "fwd-resume", "forwarder", b"fwd-resume-token").await;
    insert_token(&pool, "rcv-resume", "receiver", b"rcv-resume-token").await;

    // Connect forwarder and send 3 events
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-resume-token").await.unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-resume".to_owned(),
        reader_ips: vec!["10.2.0.1".to_owned()],
        resume: vec![],
    })).await.unwrap();
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
                reader_ip: "10.2.0.1".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_read_line: format!("LINE_{}", seq),
                read_type: "RAW".to_owned(),
            }],
        })).await.unwrap();
        fwd.recv_message().await.unwrap();
    }

    // Receiver connects with resume cursor at seq=2 (already has seqs 1 and 2)
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-resume-token").await.unwrap();
    rcv.send_message(&WsMessage::ReceiverHello(ReceiverHello {
        receiver_id: "rcv-resume".to_owned(),
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-resume".to_owned(),
            reader_ip: "10.2.0.1".to_owned(),
            stream_epoch: 1,
            last_seq: 2,
        }],
    })).await.unwrap();
    match rcv.recv_message().await.unwrap() {
        WsMessage::Heartbeat(_) => {}
        other => panic!("expected Heartbeat, got {:?}", other),
    }

    // Should receive only seq=3 as replay (not 1 or 2)
    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            assert_eq!(batch.events.len(), 1, "should replay only 1 event (seq=3)");
            assert_eq!(batch.events[0].seq, 3);
            assert_eq!(batch.events[0].raw_read_line, "LINE_3");
        }
        Ok(Ok(other)) => panic!("expected ReceiverEventBatch, got {:?}", other),
        Ok(Err(e)) => panic!("{}", e),
        Err(_) => panic!("timeout waiting for replay events"),
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
    tokio::spawn(async move { axum::serve(listener, server::build_router(app_state)).await.unwrap(); });

    insert_token(&pool, "fwd-all", "forwarder", b"fwd-all-token").await;
    insert_token(&pool, "rcv-all", "receiver", b"rcv-all-token").await;

    // Connect forwarder and send 2 events
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-all-token").await.unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-all".to_owned(),
        reader_ips: vec!["10.3.0.1".to_owned()],
        resume: vec![],
    })).await.unwrap();
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
                reader_ip: "10.3.0.1".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_read_line: format!("ALL_LINE_{}", seq),
                read_type: "RAW".to_owned(),
            }],
        })).await.unwrap();
        fwd.recv_message().await.unwrap();
    }

    // Receiver with no resume cursor - subscribes to the stream
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-all-token").await.unwrap();
    rcv.send_message(&WsMessage::ReceiverHello(ReceiverHello {
        receiver_id: "rcv-all".to_owned(),
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-all".to_owned(),
            reader_ip: "10.3.0.1".to_owned(),
            stream_epoch: 1,
            last_seq: 0, // start from beginning
        }],
    })).await.unwrap();
    match rcv.recv_message().await.unwrap() {
        WsMessage::Heartbeat(_) => {}
        other => panic!("{:?}", other),
    }

    // Should receive both events as replay
    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            assert!(batch.events.len() >= 1, "should get at least 1 replayed event");
            // Events should start from seq=1 since cursor was at 0
            assert_eq!(batch.events[0].seq, 1);
        }
        Ok(Ok(other)) => panic!("expected ReceiverEventBatch, got {:?}", other),
        Ok(Err(e)) => panic!("{}", e),
        Err(_) => panic!("timeout waiting for replay events"),
    }
}
