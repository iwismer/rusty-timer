//! Integration tests for ingest-driven epoch transition semantics.
use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

const RAW_TAG_1: &str = "aa400000000123450a2a01123018455927a7";
const RAW_TAG_2: &str = "aa4000000000AABB0a2a2601010830003ffc";

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

async fn make_server(pool: sqlx::PgPool) -> std::net::SocketAddr {
    let app_state = server::AppState::new(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state))
            .await
            .unwrap();
    });
    addr
}

#[tokio::test]
async fn test_reset_only_takes_effect_on_first_new_epoch_read() {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let addr = make_server(pool.clone()).await;

    insert_token(
        &pool,
        "fwd-epoch-transition",
        "forwarder",
        b"fwd-epoch-transition-token",
    )
    .await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-epoch-transition-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-epoch-transition".to_owned(),
        reader_ips: vec!["10.44.0.1".to_owned()],
        resume: vec![],
    }))
    .await
    .unwrap();
    let session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("{:?}", other),
    };

    for (seq, raw_read_line) in [(1u64, RAW_TAG_1), (2u64, RAW_TAG_2)] {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: session.clone(),
            batch_id: format!("e1-{}", seq),
            events: vec![ReadEvent {
                forwarder_id: "fwd-epoch-transition".to_owned(),
                reader_ip: "10.44.0.1".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-18T10:00:00.000Z".to_owned(),
                raw_read_line: raw_read_line.to_owned(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }

    let streams_resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    let streams_body: serde_json::Value = streams_resp.json().await.unwrap();
    let stream_id = streams_body["streams"][0]["stream_id"]
        .as_str()
        .unwrap()
        .to_owned();

    let before_reset_resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/metrics",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(before_reset_resp.status(), 200);
    let before_reset_metrics: serde_json::Value = before_reset_resp.json().await.unwrap();
    assert_eq!(before_reset_metrics["epoch_raw_count"], 2i64);
    assert_eq!(before_reset_metrics["epoch_dedup_count"], 2i64);
    assert_eq!(before_reset_metrics["epoch_retransmit_count"], 0i64);
    assert_eq!(before_reset_metrics["unique_chips"], 2i64);

    let reset_resp = reqwest::Client::new()
        .post(format!(
            "http://{}/api/v1/streams/{}/reset-epoch",
            addr, stream_id
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(reset_resp.status(), 204);

    let maybe_epoch_reset_command = fwd.recv_message().await.unwrap();
    assert!(
        matches!(maybe_epoch_reset_command, WsMessage::EpochResetCommand(_)),
        "expected EpochResetCommand, got {:?}",
        maybe_epoch_reset_command
    );

    let after_reset_resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/metrics",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(after_reset_resp.status(), 200);
    let after_reset_metrics: serde_json::Value = after_reset_resp.json().await.unwrap();
    assert_eq!(after_reset_metrics["epoch_raw_count"], 2i64);
    assert_eq!(after_reset_metrics["epoch_dedup_count"], 2i64);
    assert_eq!(after_reset_metrics["epoch_retransmit_count"], 0i64);
    assert_eq!(after_reset_metrics["unique_chips"], 2i64);

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: session,
        batch_id: "e2-1".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-epoch-transition".to_owned(),
            reader_ip: "10.44.0.1".to_owned(),
            stream_epoch: 2,
            seq: 1,
            reader_timestamp: "2026-02-18T10:01:00.000Z".to_owned(),
            raw_read_line: RAW_TAG_2.to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let streams_after_transition_resp = reqwest::get(format!("http://{}/api/v1/streams", addr))
        .await
        .unwrap();
    let streams_after_transition: serde_json::Value =
        streams_after_transition_resp.json().await.unwrap();
    assert_eq!(streams_after_transition["streams"][0]["stream_epoch"], 2i64);

    let after_transition_resp = reqwest::get(format!(
        "http://{}/api/v1/streams/{}/metrics",
        addr, stream_id
    ))
    .await
    .unwrap();
    assert_eq!(after_transition_resp.status(), 200);
    let after_transition_metrics: serde_json::Value = after_transition_resp.json().await.unwrap();
    assert_eq!(after_transition_metrics["epoch_raw_count"], 1i64);
    assert_eq!(after_transition_metrics["epoch_dedup_count"], 1i64);
    assert_eq!(after_transition_metrics["epoch_retransmit_count"], 0i64);
    assert_eq!(after_transition_metrics["unique_chips"], 1i64);
}
