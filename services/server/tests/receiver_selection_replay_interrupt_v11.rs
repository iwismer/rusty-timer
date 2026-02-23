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
