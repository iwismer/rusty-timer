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

#[tokio::test]
async fn resume_policy_replays_from_persisted_cursor_state() {
    let (pool, addr) = start_server().await;
    insert_token(&pool, "fwd-bf", "forwarder", b"fwd-bf-token").await;
    insert_token(&pool, "rcv-bf", "receiver", b"rcv-bf-token").await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-bf-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-bf".to_owned(),
        reader_ips: vec!["10.50.0.1:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {:?}", other),
    };

    for (seq, line) in [(1_u64, "S1"), (2_u64, "S2"), (3_u64, "S3")] {
        fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
            session_id: fwd_session.clone(),
            batch_id: format!("b{seq}"),
            events: vec![ReadEvent {
                forwarder_id: "fwd-bf".to_owned(),
                reader_ip: "10.50.0.1:10000".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-02-23T10:00:00.000Z".to_owned(),
                raw_frame: line.as_bytes().to_vec(),
                read_type: "RAW".to_owned(),
            }],
        }))
        .await
        .unwrap();
        fwd.recv_message().await.unwrap();
    }

    let stream_id: sqlx::types::Uuid = sqlx::query_scalar(
        "SELECT stream_id FROM streams WHERE forwarder_id = 'fwd-bf' AND reader_ip = '10.50.0.1:10000'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO receiver_cursors (receiver_id, stream_id, stream_epoch, last_seq)
         VALUES ('rcv-bf', $1, 1, 1)
         ON CONFLICT (receiver_id, stream_id, stream_epoch) DO UPDATE SET last_seq = 1",
    )
    .bind(stream_id)
    .execute(&pool)
    .await
    .unwrap();

    let rcv_url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-bf-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-bf".to_owned(),
        selection: ReceiverSelection::Manual {
            streams: vec![StreamRef {
                forwarder_id: "fwd-bf".to_owned(),
                reader_ip: "10.50.0.1:10000".to_owned(),
            }],
        },
        replay_policy: ReplayPolicy::Resume,
        replay_targets: None,
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
            "expected selection_applied + replay batch, got {:?} / {:?}",
            a, b
        ),
    };

    let seqs = replay_batch
        .events
        .iter()
        .map(|event| event.seq)
        .collect::<Vec<_>>();
    assert_eq!(seqs, vec![2, 3]);
}
