use rt_protocol::*;
use rt_test_utils::MockWsClient;
use server::state::ReceiverSelectionSnapshot;
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

async fn start_server() -> (sqlx::PgPool, std::net::SocketAddr, server::AppState) {
    let container = Postgres::default().start().await.unwrap();
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    std::mem::forget(container);
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{}/postgres", port);
    let pool = server::db::create_pool(&db_url).await;
    server::db::run_migrations(&pool).await;
    let app_state = server::AppState::new(pool.clone());
    let app_state_for_test = app_state.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });
    (pool, addr, app_state_for_test)
}

async fn wait_for_session_unregistered(
    state: &server::AppState,
    session_id: &str,
    timeout: Duration,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if state.get_receiver_session(session_id).await.is_none() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "session {session_id} still registered after {:?}",
            timeout
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn wait_for_session_selection(
    state: &server::AppState,
    session_id: &str,
    expected: &ReceiverSelectionSnapshot,
    timeout: Duration,
) {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let selection = state
            .get_receiver_session(session_id)
            .await
            .expect("session should stay registered")
            .selection;
        if &selection == expected {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "session {session_id} selection did not reach expected value within {:?}; last={selection:?}",
            timeout
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn test_ws_v11_route_requires_receiver_hello_v11() {
    let (pool, addr, _state) = start_server().await;
    insert_token(&pool, "rcv-v11", "receiver", b"rcv-v11-token").await;

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut client = MockWsClient::connect_with_token(&url, "rcv-v11-token")
        .await
        .unwrap();

    client
        .send_message(&WsMessage::ReceiverHello(ReceiverHello {
            receiver_id: "rcv-v11".to_owned(),
            resume: vec![],
        }))
        .await
        .unwrap();

    match client.recv_message().await.unwrap() {
        WsMessage::Error(err) => {
            assert_eq!(err.code, error_codes::PROTOCOL_ERROR);
            assert!(err.message.contains("receiver_hello_v11"));
        }
        other => panic!("expected protocol error, got {:?}", other),
    }
}

#[tokio::test]
async fn test_v11_hello_and_set_selection_emit_selection_applied_and_replace_streams() {
    let (pool, addr, state) = start_server().await;
    insert_token(&pool, "fwd-v11", "forwarder", b"fwd-v11-token").await;
    insert_token(&pool, "rcv-v11", "receiver", b"rcv-v11-token").await;

    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-v11-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-v11".to_owned(),
        reader_ips: vec!["10.20.0.1:10000".to_owned(), "10.20.0.2:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {:?}", other),
    };

    let rcv_url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-v11-token")
        .await
        .unwrap();
    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-v11".to_owned(),
        selection: ReceiverSelection::Manual {
            streams: vec![StreamRef {
                forwarder_id: "fwd-v11".to_owned(),
                reader_ip: "10.20.0.1:10000".to_owned(),
            }],
        },
        replay_policy: ReplayPolicy::LiveOnly,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let session_id = match rcv.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => {
            assert_eq!(h.device_id, "rcv-v11");
            h.session_id
        }
        other => panic!("expected heartbeat, got {:?}", other),
    };

    match rcv.recv_message().await.unwrap() {
        WsMessage::ReceiverSelectionApplied(applied) => {
            assert_eq!(applied.replay_policy, ReplayPolicy::LiveOnly);
            assert_eq!(applied.resolved_target_count, 1);
            match applied.selection {
                ReceiverSelection::Manual { streams } => {
                    assert_eq!(streams.len(), 1);
                    assert_eq!(streams[0].reader_ip, "10.20.0.1:10000");
                }
                other => panic!("expected normalized manual selection, got {:?}", other),
            }
        }
        other => panic!("expected receiver_selection_applied, got {:?}", other),
    }

    let expected_initial = ReceiverSelectionSnapshot::Manual {
        streams: vec![StreamRef {
            forwarder_id: "fwd-v11".to_owned(),
            reader_ip: "10.20.0.1:10000".to_owned(),
        }],
    };
    wait_for_session_selection(
        &state,
        &session_id,
        &expected_initial,
        Duration::from_secs(1),
    )
    .await;
    let initial_snapshot = state
        .get_receiver_session(&session_id)
        .await
        .expect("session should be tracked after hello");
    assert_eq!(initial_snapshot.selection, expected_initial);

    // Replace selection with reader2.
    rcv.send_message(&WsMessage::ReceiverSetSelection(ReceiverSetSelection {
        selection: ReceiverSelection::Manual {
            streams: vec![StreamRef {
                forwarder_id: "fwd-v11".to_owned(),
                reader_ip: "10.20.0.2:10000".to_owned(),
            }],
        },
        replay_policy: ReplayPolicy::LiveOnly,
        replay_targets: None,
    }))
    .await
    .unwrap();

    match rcv.recv_message().await.unwrap() {
        WsMessage::ReceiverSelectionApplied(applied) => {
            assert_eq!(applied.resolved_target_count, 1);
            match applied.selection {
                ReceiverSelection::Manual { streams } => {
                    assert_eq!(streams[0].reader_ip, "10.20.0.2:10000");
                }
                other => panic!("expected manual selection, got {:?}", other),
            }
        }
        other => panic!("expected receiver_selection_applied, got {:?}", other),
    }

    let expected_updated = ReceiverSelectionSnapshot::Manual {
        streams: vec![StreamRef {
            forwarder_id: "fwd-v11".to_owned(),
            reader_ip: "10.20.0.2:10000".to_owned(),
        }],
    };
    wait_for_session_selection(
        &state,
        &session_id,
        &expected_updated,
        Duration::from_secs(1),
    )
    .await;
    let updated_snapshot = state
        .get_receiver_session(&session_id)
        .await
        .expect("session should still be tracked after set_selection");
    assert_eq!(updated_snapshot.selection, expected_updated);

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Send one event on each stream; receiver should only get reader2 after replacement.
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session.clone(),
        batch_id: "b-old".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-v11".to_owned(),
            reader_ip: "10.20.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-22T10:00:00.000Z".to_owned(),
            raw_read_line: "OLD_STREAM".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "b-new".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-v11".to_owned(),
            reader_ip: "10.20.0.2:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-22T10:00:00.000Z".to_owned(),
            raw_read_line: "NEW_STREAM".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            assert_eq!(batch.events.len(), 1);
            assert_eq!(batch.events[0].reader_ip, "10.20.0.2:10000");
            assert_eq!(batch.events[0].raw_read_line, "NEW_STREAM");
        }
        Ok(Ok(other)) => panic!("expected receiver_event_batch, got {:?}", other),
        Ok(Err(e)) => panic!("recv error: {}", e),
        Err(_) => panic!("timeout waiting for event batch"),
    }
}

#[tokio::test]
async fn test_v11_disconnect_unregisters_receiver_session() {
    let (pool, addr, state) = start_server().await;
    insert_token(&pool, "rcv-v11", "receiver", b"rcv-v11-token").await;

    let rcv_url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, "rcv-v11-token")
        .await
        .unwrap();
    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-v11".to_owned(),
        selection: ReceiverSelection::Manual {
            streams: vec![StreamRef {
                forwarder_id: "unknown-fwd".to_owned(),
                reader_ip: "10.20.0.99:10000".to_owned(),
            }],
        },
        replay_policy: ReplayPolicy::LiveOnly,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let session_id = match rcv.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {:?}", other),
    };
    let _ = rcv.recv_message().await.unwrap();

    assert!(
        state.get_receiver_session(&session_id).await.is_some(),
        "session should be tracked while connected"
    );

    drop(rcv);
    wait_for_session_unregistered(&state, &session_id, Duration::from_secs(2)).await;
}
