//! Integration tests for receiver WS subscribe and real-time event delivery.
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

#[tokio::test]
async fn test_receiver_connect_and_heartbeat() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, server::build_router(app_state)).await.unwrap(); });

    insert_token(&pool, "rcv-001", "receiver", b"rcv-token-001").await;
    let url = format!("ws://{}/ws/v1/receivers", addr);
    let mut client = MockWsClient::connect_with_token(&url, "rcv-token-001").await.unwrap();
    client.send_message(&WsMessage::ReceiverHello(ReceiverHello { receiver_id: "rcv-001".to_owned(), resume: vec![] })).await.unwrap();
    match client.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => {
            assert_eq!(h.device_id, "rcv-001");
            assert!(!h.session_id.is_empty());
        }
        other => panic!("expected Heartbeat, got {:?}", other),
    }
}

#[tokio::test]
async fn test_receiver_invalid_token_rejected() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, server::build_router(app_state)).await.unwrap(); });

    let url = format!("ws://{}/ws/v1/receivers", addr);
    let mut client = MockWsClient::connect_with_token(&url, "bad-token").await.unwrap();
    client.send_message(&WsMessage::ReceiverHello(ReceiverHello { receiver_id: "rcv-unknown".to_owned(), resume: vec![] })).await.unwrap();
    match client.recv_message().await {
        Ok(WsMessage::Error(e)) => { assert_eq!(e.code, error_codes::INVALID_TOKEN); }
        Err(_) => {}
        Ok(other) => panic!("expected INVALID_TOKEN error, got {:?}", other),
    }
}

#[tokio::test]
async fn test_receiver_receives_realtime_events() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, server::build_router(app_state)).await.unwrap(); });

    insert_token(&pool, "fwd-rt", "forwarder", b"fwd-rt-token").await;
    insert_token(&pool, "rcv-rt", "receiver", b"rcv-rt-token").await;

    // Connect forwarder
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-rt-token").await.unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-rt".to_owned(),
        reader_ips: vec!["10.0.0.1".to_owned()],
        resume: vec![],
    })).await.unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    // Connect receiver - subscribe to the stream
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-rt-token").await.unwrap();
    rcv.send_message(&WsMessage::ReceiverHello(ReceiverHello { receiver_id: "rcv-rt".to_owned(), resume: vec![] })).await.unwrap();
    match rcv.recv_message().await.unwrap() {
        WsMessage::Heartbeat(_) => {}
        other => panic!("{:?}", other),
    }
    // Subscribe to the stream
    rcv.send_message(&WsMessage::ReceiverSubscribe(ReceiverSubscribe {
        session_id: String::new(),
        streams: vec![StreamRef { forwarder_id: "fwd-rt".to_owned(), reader_ip: "10.0.0.1".to_owned() }],
    })).await.unwrap();

    // Give subscription a moment to register
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Forwarder sends an event
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session.clone(),
        batch_id: "b1".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-rt".to_owned(),
            reader_ip: "10.0.0.1".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_read_line: "RT_LINE_1".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    })).await.unwrap();
    fwd.recv_message().await.unwrap(); // ack

    // Receiver should get the event within reasonable time
    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            assert_eq!(batch.events.len(), 1);
            assert_eq!(batch.events[0].raw_read_line, "RT_LINE_1");
            assert_eq!(batch.events[0].seq, 1);
        }
        Ok(Ok(other)) => panic!("expected ReceiverEventBatch, got {:?}", other),
        Ok(Err(e)) => panic!("recv error: {}", e),
        Err(_) => panic!("timeout waiting for receiver event"),
    }
}

#[tokio::test]
async fn test_receiver_ack_updates_cursor() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let app_state = server::AppState::new(pool.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, server::build_router(app_state)).await.unwrap(); });

    insert_token(&pool, "fwd-ack", "forwarder", b"fwd-ack-token").await;
    insert_token(&pool, "rcv-ack", "receiver", b"rcv-ack-token").await;

    // Connect forwarder
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-ack-token").await.unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-ack".to_owned(),
        reader_ips: vec!["10.1.0.1".to_owned()],
        resume: vec![],
    })).await.unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    // Connect receiver
    let rcv_url = format!("ws://{}/ws/v1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-ack-token").await.unwrap();
    rcv.send_message(&WsMessage::ReceiverHello(ReceiverHello { receiver_id: "rcv-ack".to_owned(), resume: vec![] })).await.unwrap();
    let rcv_session = match rcv.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };
    rcv.send_message(&WsMessage::ReceiverSubscribe(ReceiverSubscribe {
        session_id: rcv_session.clone(),
        streams: vec![StreamRef { forwarder_id: "fwd-ack".to_owned(), reader_ip: "10.1.0.1".to_owned() }],
    })).await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send event
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "b1".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-ack".to_owned(),
            reader_ip: "10.1.0.1".to_owned(),
            stream_epoch: 1,
            seq: 5,
            reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
            raw_read_line: "ACK_LINE".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    })).await.unwrap();
    fwd.recv_message().await.unwrap();

    // Receiver gets event
    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            // Ack it
            let entry = &batch.events[0];
            rcv.send_message(&WsMessage::ReceiverAck(ReceiverAck {
                session_id: rcv_session.clone(),
                entries: vec![AckEntry {
                    forwarder_id: entry.forwarder_id.clone(),
                    reader_ip: entry.reader_ip.clone(),
                    stream_epoch: entry.stream_epoch,
                    last_seq: entry.seq,
                }],
            })).await.unwrap();
        }
        Ok(Ok(other)) => panic!("expected ReceiverEventBatch, got {:?}", other),
        Ok(Err(e)) => panic!("{}", e),
        Err(_) => panic!("timeout"),
    }

    // After ack, cursor should be persisted in DB
    tokio::time::sleep(Duration::from_millis(100)).await;
    let row = sqlx::query!(
        "SELECT last_seq FROM receiver_cursors WHERE receiver_id = 'rcv-ack'"
    ).fetch_one(&pool).await.unwrap();
    assert_eq!(row.last_seq, 5);
}
