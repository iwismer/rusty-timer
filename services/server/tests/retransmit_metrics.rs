//! Tests for retransmit/dedupe metrics invariants.
use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
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
async fn test_metrics_invariant_maintained() {
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
    insert_token(&pool, "fwd-metrics", "forwarder", b"metrics-token").await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect_with_token(&url, "metrics-token")
        .await
        .unwrap();
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-metrics".to_owned(),
            reader_ips: vec!["10.1.1.1:10000".to_owned()],
            display_name: None,
        }))
        .await
        .unwrap();
    let session_id = match client.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };
    let make_event = |seq: u64| ReadEvent {
        forwarder_id: "fwd-metrics".to_owned(),
        reader_ip: "10.1.1.1:10000".to_owned(),
        stream_epoch: 1,
        seq,
        reader_timestamp: format!("2026-02-17T10:00:0{}.000Z", seq),
        raw_read_line: format!("LINE_{}", seq),
        read_type: "RAW".to_owned(),
    };
    for seq in 1..=3u64 {
        client
            .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
                session_id: session_id.clone(),
                batch_id: format!("b{}", seq),
                events: vec![make_event(seq)],
            }))
            .await
            .unwrap();
        client.recv_message().await.unwrap();
    }
    client
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session_id.clone(),
            batch_id: "r1".to_owned(),
            events: vec![make_event(1)],
        }))
        .await
        .unwrap();
    client.recv_message().await.unwrap();
    for i in 0..2u32 {
        client
            .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
                session_id: session_id.clone(),
                batch_id: format!("r2-{}", i),
                events: vec![make_event(2)],
            }))
            .await
            .unwrap();
        client.recv_message().await.unwrap();
    }
    let row = sqlx::query!("SELECT raw_count, dedup_count, retransmit_count FROM stream_metrics")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(row.raw_count, 6);
    assert_eq!(row.dedup_count, 3);
    assert_eq!(row.retransmit_count, 3);
    assert_eq!(row.raw_count, row.dedup_count + row.retransmit_count);
    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(event_count, 3);
}

#[tokio::test]
async fn test_multi_stream_metrics_independent() {
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
    insert_token(&pool, "fwd-multi", "forwarder", b"multi-stream-token").await;
    let url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut client = MockWsClient::connect_with_token(&url, "multi-stream-token")
        .await
        .unwrap();
    client
        .send_message(&WsMessage::ForwarderHello(ForwarderHello {
            forwarder_id: "fwd-multi".to_owned(),
            reader_ips: vec!["10.2.2.1:10000".to_owned(), "10.2.2.2:10000".to_owned()],
            display_name: None,
        }))
        .await
        .unwrap();
    let session_id = match client.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };
    for seq in 1..=2u64 {
        client
            .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
                session_id: session_id.clone(),
                batch_id: format!("s1-{}", seq),
                events: vec![ReadEvent {
                    forwarder_id: "fwd-multi".to_owned(),
                    reader_ip: "10.2.2.1:10000".to_owned(),
                    stream_epoch: 1,
                    seq,
                    reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
                    raw_read_line: format!("S1_{}", seq),
                    read_type: "RAW".to_owned(),
                }],
            }))
            .await
            .unwrap();
        client.recv_message().await.unwrap();
    }
    let e2 = ReadEvent {
        forwarder_id: "fwd-multi".to_owned(),
        reader_ip: "10.2.2.2:10000".to_owned(),
        stream_epoch: 1,
        seq: 1,
        reader_timestamp: "2026-02-17T10:00:00.000Z".to_owned(),
        raw_read_line: "S2_1".to_owned(),
        read_type: "RAW".to_owned(),
    };
    client
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session_id.clone(),
            batch_id: "s2-1".to_owned(),
            events: vec![e2.clone()],
        }))
        .await
        .unwrap();
    client.recv_message().await.unwrap();
    client
        .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session_id.clone(),
            batch_id: "s2-r".to_owned(),
            events: vec![e2],
        }))
        .await
        .unwrap();
    client.recv_message().await.unwrap();
    let s1 = sqlx::query!(r#"SELECT sm.raw_count, sm.dedup_count, sm.retransmit_count FROM stream_metrics sm JOIN streams s ON s.stream_id = sm.stream_id WHERE s.reader_ip = '10.2.2.1:10000'"#).fetch_one(&pool).await.unwrap();
    assert_eq!(s1.raw_count, 2);
    assert_eq!(s1.dedup_count, 2);
    assert_eq!(s1.retransmit_count, 0);
    let s2 = sqlx::query!(r#"SELECT sm.raw_count, sm.dedup_count, sm.retransmit_count FROM stream_metrics sm JOIN streams s ON s.stream_id = sm.stream_id WHERE s.reader_ip = '10.2.2.2:10000'"#).fetch_one(&pool).await.unwrap();
    assert_eq!(s2.raw_count, 2);
    assert_eq!(s2.dedup_count, 1);
    assert_eq!(s2.retransmit_count, 1);
}
