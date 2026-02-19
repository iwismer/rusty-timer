//! Integration tests for forwarder ingest WS endpoint and dedupe storage.
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

#[tokio::test]
async fn test_first_insert_stores_event_and_acks() {
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
    insert_token(&pool, "fwd-001", "forwarder", b"test-token-001").await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect_with_token(&url, "test-token-001")
        .await
        .unwrap();
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-001".to_owned(),
            reader_ips: vec!["192.168.1.10:10000".to_owned()],
            resume: vec![],
            display_name: None,
        }))
        .await
        .unwrap();
    let session_id = match client.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected Heartbeat, got {:?}", other),
    };
    client
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session_id.clone(),
            batch_id: "b001".to_owned(),
            events: vec![ReadEvent {
                forwarder_id: "fwd-001".to_owned(),
                reader_ip: "192.168.1.10:10000".to_owned(),
                stream_epoch: 1,
                seq: 1,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_read_line: "LINE1".to_owned(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
    match client.recv_message().await.unwrap() {
        WsMessage::ForwarderAck(a) => {
            assert_eq!(a.session_id, session_id);
            assert_eq!(a.entries.len(), 1);
            assert_eq!(a.entries[0].last_seq, 1);
        }
        other => panic!("expected ForwarderAck, got {:?}", other),
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "event should be stored in DB");
}

#[tokio::test]
async fn test_identical_retransmit_no_dup() {
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
    insert_token(&pool, "fwd-002", "forwarder", b"test-token-002").await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect_with_token(&url, "test-token-002")
        .await
        .unwrap();
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-002".to_owned(),
            reader_ips: vec!["192.168.1.20:10000".to_owned()],
            resume: vec![],
            display_name: None,
        }))
        .await
        .unwrap();
    let session_id = match client.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };
    let event = ReadEvent {
        forwarder_id: "fwd-002".to_owned(),
        reader_ip: "192.168.1.20:10000".to_owned(),
        stream_epoch: 1,
        seq: 1,
        reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
        raw_read_line: "LINE1".to_owned(),
        read_type: "RAW".to_owned(),
    };
    client
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session_id.clone(),
            batch_id: "b001".to_owned(),
            events: vec![event.clone()],
        }))
        .await
        .unwrap();
    client.recv_message().await.unwrap();
    client
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session_id.clone(),
            batch_id: "b002".to_owned(),
            events: vec![event],
        }))
        .await
        .unwrap();
    client.recv_message().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "retransmit must not duplicate");
    let rt: i64 = sqlx::query_scalar("SELECT retransmit_count FROM stream_metrics")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rt, 1);
    let row = sqlx::query!("SELECT raw_count, dedup_count FROM stream_metrics")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.raw_count, 2);
    assert_eq!(row.dedup_count, 1);
}

#[tokio::test]
async fn test_mismatched_payload_rejected() {
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
    insert_token(&pool, "fwd-003", "forwarder", b"test-token-003").await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect_with_token(&url, "test-token-003")
        .await
        .unwrap();
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-003".to_owned(),
            reader_ips: vec!["192.168.1.30:10000".to_owned()],
            resume: vec![],
            display_name: None,
        }))
        .await
        .unwrap();
    let session_id = match client.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };
    client
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session_id.clone(),
            batch_id: "b001".to_owned(),
            events: vec![ReadEvent {
                forwarder_id: "fwd-003".to_owned(),
                reader_ip: "192.168.1.30:10000".to_owned(),
                stream_epoch: 1,
                seq: 1,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_read_line: "ORIGINAL_LINE".to_owned(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
    client.recv_message().await.unwrap();
    client
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session_id.clone(),
            batch_id: "b002".to_owned(),
            events: vec![ReadEvent {
                forwarder_id: "fwd-003".to_owned(),
                reader_ip: "192.168.1.30:10000".to_owned(),
                stream_epoch: 1,
                seq: 1,
                reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                raw_read_line: "DIFFERENT_LINE".to_owned(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
    let ack = match client.recv_message().await {
        Ok(WsMessage::ForwarderAck(ack)) => ack,
        Ok(other) => panic!("expected ForwarderAck, got {:?}", other),
        Err(err) => panic!("expected ForwarderAck, got recv error: {:?}", err),
    };
    assert_eq!(ack.session_id, session_id);
    assert_eq!(ack.entries.len(), 1);
    let entry = &ack.entries[0];
    assert_eq!(entry.forwarder_id, "fwd-003");
    assert_eq!(entry.reader_ip, "192.168.1.30:10000");
    assert_eq!(entry.stream_epoch, 1);
    assert_eq!(entry.last_seq, 1);
    let raw_line: String = sqlx::query_scalar("SELECT raw_read_line FROM events WHERE seq = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(raw_line, "ORIGINAL_LINE");
}

#[tokio::test]
async fn test_first_connection_wins() {
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
    insert_token(&pool, "fwd-dup", "forwarder", b"test-token-dup").await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client1 = MockWsClient::connect_with_token(&url, "test-token-dup")
        .await
        .unwrap();
    client1
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-dup".to_owned(),
            reader_ips: vec!["10.0.0.1:10000".to_owned()],
            resume: vec![],
            display_name: None,
        }))
        .await
        .unwrap();
    assert!(matches!(
        client1.recv_message().await.unwrap(),
        WsMessage::Heartbeat(_)
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let mut client2 = MockWsClient::connect_with_token(&url, "test-token-dup")
        .await
        .unwrap();
    client2
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-dup".to_owned(),
            reader_ips: vec!["10.0.0.1:10000".to_owned()],
            resume: vec![],
            display_name: None,
        }))
        .await
        .unwrap();
    match client2.recv_message().await {
        Ok(WsMessage::Error(e)) => {
            assert!(
                e.code == rt_protocol::error_codes::PROTOCOL_ERROR
                    || e.code == rt_protocol::error_codes::IDENTITY_MISMATCH
            );
        }
        Err(_) => {}
        Ok(other) => panic!("expected Error for duplicate connection, got {:?}", other),
    }
}

#[tokio::test]
async fn test_invalid_token_rejected() {
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
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect_with_token(&url, "unknown-token")
        .await
        .unwrap();
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-unknown".to_owned(),
            reader_ips: vec![],
            resume: vec![],
            display_name: None,
        }))
        .await
        .unwrap();
    match client.recv_message().await {
        Ok(WsMessage::Error(e)) => {
            assert_eq!(e.code, rt_protocol::error_codes::INVALID_TOKEN);
        }
        Err(_) => {}
        Ok(other) => panic!("expected INVALID_TOKEN error, got {:?}", other),
    }
}

#[tokio::test]
async fn test_path_unsafe_forwarder_id_rejected() {
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

    insert_token(&pool, "fwd/bad", "forwarder", b"test-token-path-unsafe").await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect_with_token(&url, "test-token-path-unsafe")
        .await
        .unwrap();
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd/bad".to_owned(),
            reader_ips: vec![],
            resume: vec![],
            display_name: None,
        }))
        .await
        .unwrap();

    match client.recv_message().await {
        Ok(WsMessage::Error(e)) => {
            assert_eq!(e.code, rt_protocol::error_codes::INVALID_TOKEN);
        }
        Err(_) => {}
        Ok(other) => panic!("expected INVALID_TOKEN error, got {:?}", other),
    }
}
