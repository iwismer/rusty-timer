use rt_protocol::*;
use rt_test_utils::MockWsClient;
use sha2::{Digest, Sha256};
use sqlx::Row;
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
    insert_token(pool, "fwd-race", "forwarder", b"fwd-race-token").await;
    let fwd_url = format!("ws://{}/ws/v1/forwarders", addr);
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, "fwd-race-token")
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: "fwd-race".to_owned(),
        reader_ips: vec!["10.30.0.1:10000".to_owned(), "10.30.0.2:10000".to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    let session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {:?}", other),
    };
    (fwd, session)
}

async fn insert_race(pool: &sqlx::PgPool, name: &str) -> String {
    sqlx::query_scalar::<_, String>("INSERT INTO races (name) VALUES ($1) RETURNING race_id::text")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn map_epoch(pool: &sqlx::PgPool, reader_ip: &str, epoch: i64, race_id: &str) {
    let stream_id: sqlx::types::Uuid = sqlx::query_scalar(
        "SELECT stream_id FROM streams WHERE forwarder_id = 'fwd-race' AND reader_ip = $1",
    )
    .bind(reader_ip)
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO stream_epoch_races (stream_id, stream_epoch, race_id)
         VALUES ($1, $2, $3::uuid)
         ON CONFLICT (stream_id, stream_epoch) DO UPDATE SET race_id = EXCLUDED.race_id",
    )
    .bind(stream_id)
    .bind(epoch)
    .bind(race_id)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn test_race_all_resolves_all_mapped_streams() {
    let (pool, addr) = start_server().await;
    let (_fwd, _fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-race", "receiver", b"rcv-race-token").await;

    let race_id = insert_race(&pool, "all").await;
    map_epoch(&pool, "10.30.0.1:10000", 1, &race_id).await;
    map_epoch(&pool, "10.30.0.2:10000", 1, &race_id).await;

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-race-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-race".to_owned(),
        selection: ReceiverSelection::Race {
            race_id: race_id.clone(),
            epoch_scope: EpochScope::All,
        },
        replay_policy: ReplayPolicy::LiveOnly,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    match rcv.recv_message().await.unwrap() {
        WsMessage::ReceiverSelectionApplied(applied) => {
            assert_eq!(applied.resolved_target_count, 2);
            match applied.selection {
                ReceiverSelection::Manual { streams } => {
                    assert_eq!(streams.len(), 2);
                }
                other => panic!("expected manual selection, got {:?}", other),
            }
        }
        other => panic!("expected selection_applied, got {:?}", other),
    }
}

#[tokio::test]
async fn test_race_current_resolves_only_current_epoch_mappings() {
    let (pool, addr) = start_server().await;
    let (_fwd, _fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-race", "receiver", b"rcv-race-token").await;

    let race_id = insert_race(&pool, "current").await;

    // Stream 1 current epoch is 1 and mapped at 1 => included.
    map_epoch(&pool, "10.30.0.1:10000", 1, &race_id).await;
    // Stream 2 mapped only at epoch 1, then current epoch advanced to 2 => excluded for current.
    map_epoch(&pool, "10.30.0.2:10000", 1, &race_id).await;
    sqlx::query(
        "UPDATE streams SET stream_epoch = 2 WHERE forwarder_id = 'fwd-race' AND reader_ip = '10.30.0.2:10000'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-race-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-race".to_owned(),
        selection: ReceiverSelection::Race {
            race_id,
            epoch_scope: EpochScope::Current,
        },
        replay_policy: ReplayPolicy::LiveOnly,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    match rcv.recv_message().await.unwrap() {
        WsMessage::ReceiverSelectionApplied(applied) => {
            assert_eq!(applied.resolved_target_count, 1);
            match applied.selection {
                ReceiverSelection::Manual { streams } => {
                    assert_eq!(streams.len(), 1);
                    assert_eq!(streams[0].reader_ip, "10.30.0.1:10000");
                }
                other => panic!("expected manual selection, got {:?}", other),
            }
        }
        other => panic!("expected selection_applied, got {:?}", other),
    }
}

#[tokio::test]
async fn test_race_current_resume_does_not_backfill_prior_epoch() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-race", "receiver", b"rcv-race-token").await;

    // Persist one old-epoch event for reader1.
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session.clone(),
        batch_id: "e1".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-race".to_owned(),
            reader_ip: "10.30.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-22T10:00:00.000Z".to_owned(),
            raw_read_line: "OLD_EPOCH".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let race_id = insert_race(&pool, "current-resume").await;
    map_epoch(&pool, "10.30.0.1:10000", 2, &race_id).await;
    sqlx::query(
        "UPDATE streams SET stream_epoch = 2 WHERE forwarder_id = 'fwd-race' AND reader_ip = '10.30.0.1:10000'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-race-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-race".to_owned(),
        selection: ReceiverSelection::Race {
            race_id,
            epoch_scope: EpochScope::Current,
        },
        replay_policy: ReplayPolicy::Resume,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    let _ = rcv.recv_message().await.unwrap();

    // No old epoch replay should arrive.
    match tokio::time::timeout(Duration::from_millis(400), rcv.recv_message()).await {
        Err(_) => {}
        Ok(Ok(other)) => panic!("expected no replay before live events, got {:?}", other),
        Ok(Err(_)) => {}
    }

    // A new current-epoch event should still be delivered live.
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "e2".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-race".to_owned(),
            reader_ip: "10.30.0.1:10000".to_owned(),
            stream_epoch: 2,
            seq: 1,
            reader_timestamp: "2026-02-22T10:00:01.000Z".to_owned(),
            raw_read_line: "CURRENT_EPOCH".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            let event = &batch.events[0];
            assert_eq!(event.stream_epoch, 2);
            assert_eq!(event.raw_read_line, "CURRENT_EPOCH");
        }
        Ok(Ok(other)) => panic!("expected receiver_event_batch, got {:?}", other),
        Ok(Err(e)) => panic!("recv error: {}", e),
        Err(_) => panic!("timeout waiting for live event"),
    }

    let row = sqlx::query(
        "SELECT stream_epoch FROM streams WHERE forwarder_id = 'fwd-race' AND reader_ip = '10.30.0.1:10000'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<i64, _>("stream_epoch"), 2);
}

#[tokio::test]
async fn test_race_current_resume_without_cursor_accepts_live_events_after_stale_epoch_jump() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-race", "receiver", b"rcv-race-token").await;

    // Persist one event in epoch 1 before stream_epoch is moved forward.
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session.clone(),
        batch_id: "seed".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-race".to_owned(),
            reader_ip: "10.30.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-22T10:00:00.000Z".to_owned(),
            raw_read_line: "SEED_E1".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    // Simulate stream_epoch being advanced ahead of real incoming events.
    sqlx::query(
        "UPDATE streams SET stream_epoch = 2 WHERE forwarder_id = 'fwd-race' AND reader_ip = '10.30.0.1:10000'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let race_id = insert_race(&pool, "current-stale-epoch").await;
    map_epoch(&pool, "10.30.0.1:10000", 2, &race_id).await;

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-race-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-race".to_owned(),
        selection: ReceiverSelection::Race {
            race_id,
            epoch_scope: EpochScope::Current,
        },
        replay_policy: ReplayPolicy::Resume,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    let _ = rcv.recv_message().await.unwrap();

    // No persisted cursor exists; new live events should still flow.
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "live-after-subscribe".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-race".to_owned(),
            reader_ip: "10.30.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 2,
            reader_timestamp: "2026-02-22T10:00:01.000Z".to_owned(),
            raw_read_line: "LIVE_E1".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            let event = &batch.events[0];
            assert_eq!(event.stream_epoch, 1);
            assert_eq!(event.seq, 2);
            assert_eq!(event.raw_read_line, "LIVE_E1");
        }
        Ok(Ok(other)) => panic!("expected receiver_event_batch, got {:?}", other),
        Ok(Err(e)) => panic!("recv error: {}", e),
        Err(_) => panic!("timeout waiting for live event"),
    }
}

#[tokio::test]
async fn test_race_current_resume_without_cursor_replays_persisted_current_epoch() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-race", "receiver", b"rcv-race-token").await;

    // Persist an old-epoch event first.
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session.clone(),
        batch_id: "old".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-race".to_owned(),
            reader_ip: "10.30.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-22T10:00:00.000Z".to_owned(),
            raw_read_line: "OLD_E1".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    // Move stream_epoch and persist a current-epoch event.
    sqlx::query(
        "UPDATE streams SET stream_epoch = 2 WHERE forwarder_id = 'fwd-race' AND reader_ip = '10.30.0.1:10000'",
    )
    .execute(&pool)
    .await
    .unwrap();
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "current".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-race".to_owned(),
            reader_ip: "10.30.0.1:10000".to_owned(),
            stream_epoch: 2,
            seq: 1,
            reader_timestamp: "2026-02-22T10:00:01.000Z".to_owned(),
            raw_read_line: "CURRENT_E2".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let race_id = insert_race(&pool, "current-resume-replay").await;
    map_epoch(&pool, "10.30.0.1:10000", 2, &race_id).await;

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-race-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-race".to_owned(),
        selection: ReceiverSelection::Race {
            race_id,
            epoch_scope: EpochScope::Current,
        },
        replay_policy: ReplayPolicy::Resume,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
            assert_eq!(batch.events.len(), 1);
            let event = &batch.events[0];
            assert_eq!(event.stream_epoch, 2);
            assert_eq!(event.seq, 1);
            assert_eq!(event.raw_read_line, "CURRENT_E2");
        }
        Ok(Ok(other)) => panic!("expected receiver_event_batch replay, got {:?}", other),
        Ok(Err(e)) => panic!("recv error: {}", e),
        Err(_) => panic!("timeout waiting for replayed current-epoch event"),
    }
}

#[tokio::test]
async fn test_race_current_live_events_are_filtered_after_epoch_moves_out_of_scope() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-race", "receiver", b"rcv-race-token").await;

    let race_id = insert_race(&pool, "current-live-filter").await;
    map_epoch(&pool, "10.30.0.1:10000", 1, &race_id).await;

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-race-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-race".to_owned(),
        selection: ReceiverSelection::Race {
            race_id,
            epoch_scope: EpochScope::Current,
        },
        replay_policy: ReplayPolicy::LiveOnly,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    let _ = rcv.recv_message().await.unwrap();

    sqlx::query(
        "UPDATE streams SET stream_epoch = 2 WHERE forwarder_id = 'fwd-race' AND reader_ip = '10.30.0.1:10000'",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Give server refresh loop enough time to re-resolve current-epoch race targets.
    tokio::time::sleep(Duration::from_millis(700)).await;

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "stale-epoch-live".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-race".to_owned(),
            reader_ip: "10.30.0.1:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-23T10:00:01.000Z".to_owned(),
            raw_read_line: "STALE_EPOCH".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_millis(900);
    while tokio::time::Instant::now() < deadline {
        let msg = match tokio::time::timeout(Duration::from_millis(250), rcv.recv_message()).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(_)) => break,
            Err(_) => continue,
        };

        match msg {
            WsMessage::Heartbeat(_) | WsMessage::ReceiverSelectionApplied(_) => {}
            WsMessage::ReceiverEventBatch(batch) => {
                panic!(
                    "expected stale event to be filtered, received event batch: {:?}",
                    batch
                );
            }
            other => panic!("expected no stale event batches, got {:?}", other),
        }
    }
}

#[tokio::test]
async fn test_race_current_refresh_not_starved_under_continuous_live_traffic() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-race", "receiver", b"rcv-race-token").await;

    let race_id = insert_race(&pool, "current-refresh-under-load").await;
    map_epoch(&pool, "10.30.0.1:10000", 1, &race_id).await;

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-race-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-race".to_owned(),
        selection: ReceiverSelection::Race {
            race_id,
            epoch_scope: EpochScope::Current,
        },
        replay_policy: ReplayPolicy::LiveOnly,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    let _ = rcv.recv_message().await.unwrap();

    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
    let traffic_task = tokio::spawn(async move {
        let mut seq: u64 = 1;
        loop {
            if *stop_rx.borrow() {
                break;
            }

            for _ in 0..24 {
                if fwd
                    .send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
                        session_id: fwd_session.clone(),
                        batch_id: format!("steady-{seq}"),
                        events: vec![ReadEvent {
                            forwarder_id: "fwd-race".to_owned(),
                            reader_ip: "10.30.0.1:10000".to_owned(),
                            stream_epoch: 1,
                            seq,
                            reader_timestamp: "2026-02-23T10:00:01.000Z".to_owned(),
                            raw_read_line: format!("STEADY_{seq}"),
                            read_type: "RAW".to_owned(),
                        }],
                    }))
                    .await
                    .is_err()
                {
                    return;
                }
                seq += 1;
            }

            tokio::select! {
                _ = stop_rx.changed() => {}
                _ = tokio::time::sleep(Duration::from_millis(2)) => {}
            }
        }
    });

    let pre_update_deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_pre_update_batch = false;
    while tokio::time::Instant::now() < pre_update_deadline {
        let msg = match tokio::time::timeout(Duration::from_millis(250), rcv.recv_message()).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => panic!("recv error before update: {}", e),
            Err(_) => continue,
        };
        match msg {
            WsMessage::ReceiverEventBatch(batch) => {
                if !batch.events.is_empty() {
                    saw_pre_update_batch = true;
                    break;
                }
            }
            WsMessage::Heartbeat(_) | WsMessage::ReceiverSelectionApplied(_) => {}
            other => panic!("unexpected message before update: {:?}", other),
        }
    }
    assert!(
        saw_pre_update_batch,
        "expected receiver to receive live traffic before stream_epoch update"
    );

    sqlx::query(
        "UPDATE streams SET stream_epoch = 2 WHERE forwarder_id = 'fwd-race' AND reader_ip = '10.30.0.1:10000'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let refresh_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    let mut saw_empty_refresh = false;
    while tokio::time::Instant::now() < refresh_deadline {
        let msg = match tokio::time::timeout(Duration::from_millis(250), rcv.recv_message()).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => panic!("recv error while waiting for refresh: {}", e),
            Err(_) => continue,
        };
        match msg {
            WsMessage::ReceiverSelectionApplied(applied) => {
                if applied.resolved_target_count == 0 {
                    saw_empty_refresh = true;
                    break;
                }
            }
            WsMessage::ReceiverEventBatch(_) | WsMessage::Heartbeat(_) => {}
            other => panic!("unexpected message while waiting for refresh: {:?}", other),
        }
    }

    assert!(
        saw_empty_refresh,
        "expected current-scope refresh to resolve zero targets under sustained traffic"
    );

    let no_event_deadline = tokio::time::Instant::now() + Duration::from_millis(900);
    while tokio::time::Instant::now() < no_event_deadline {
        let msg = match tokio::time::timeout(Duration::from_millis(250), rcv.recv_message()).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(_)) => break,
            Err(_) => continue,
        };
        match msg {
            WsMessage::Heartbeat(_) | WsMessage::ReceiverSelectionApplied(_) => {}
            WsMessage::ReceiverEventBatch(batch) => {
                panic!(
                    "expected stale live traffic to be filtered after refresh, got {:?}",
                    batch
                );
            }
            other => panic!("unexpected message after refresh: {:?}", other),
        }
    }

    let _ = stop_tx.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(2), traffic_task).await;
}

#[tokio::test]
async fn test_race_current_selection_refreshes_to_new_stream_mappings_without_set_selection() {
    let (pool, addr) = start_server().await;
    let (mut fwd, fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-race", "receiver", b"rcv-race-token").await;

    let race_id = insert_race(&pool, "dynamic-refresh").await;
    map_epoch(&pool, "10.30.0.1:10000", 1, &race_id).await;

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-race-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-race".to_owned(),
        selection: ReceiverSelection::Race {
            race_id: race_id.clone(),
            epoch_scope: EpochScope::Current,
        },
        replay_policy: ReplayPolicy::LiveOnly,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    match rcv.recv_message().await.unwrap() {
        WsMessage::ReceiverSelectionApplied(applied) => {
            assert_eq!(applied.resolved_target_count, 1);
            match applied.selection {
                ReceiverSelection::Manual { streams } => {
                    assert_eq!(streams.len(), 1);
                    assert_eq!(streams[0].reader_ip, "10.30.0.1:10000");
                }
                other => panic!("expected manual selection, got {:?}", other),
            }
        }
        other => panic!("expected selection_applied, got {:?}", other),
    }

    map_epoch(&pool, "10.30.0.2:10000", 1, &race_id).await;
    sqlx::query(
        "UPDATE streams SET stream_epoch = 2 WHERE forwarder_id = 'fwd-race' AND reader_ip = '10.30.0.1:10000'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE streams SET stream_epoch = 1 WHERE forwarder_id = 'fwd-race' AND reader_ip = '10.30.0.2:10000'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut saw_refresh = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while tokio::time::Instant::now() < deadline {
        let msg = match tokio::time::timeout(Duration::from_millis(250), rcv.recv_message()).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => panic!("recv error: {}", e),
            Err(_) => continue,
        };
        match msg {
            WsMessage::ReceiverSelectionApplied(applied) => {
                if applied.resolved_target_count == 1 {
                    match applied.selection {
                        ReceiverSelection::Manual { streams } if streams.len() == 1 => {
                            if streams[0].reader_ip == "10.30.0.2:10000" {
                                saw_refresh = true;
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
            WsMessage::Heartbeat(_) => {}
            WsMessage::ReceiverEventBatch(_) => {}
            other => panic!("unexpected message during refresh wait: {:?}", other),
        }
    }

    assert!(
        saw_refresh,
        "expected refreshed receiver_selection_applied with reader2"
    );

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session.clone(),
        batch_id: "r1-live".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-race".to_owned(),
            reader_ip: "10.30.0.1:10000".to_owned(),
            stream_epoch: 2,
            seq: 1,
            reader_timestamp: "2026-02-23T10:00:02.000Z".to_owned(),
            raw_read_line: "R1_AFTER_REFRESH".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: fwd_session,
        batch_id: "r2-live".to_owned(),
        events: vec![ReadEvent {
            forwarder_id: "fwd-race".to_owned(),
            reader_ip: "10.30.0.2:10000".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "2026-02-23T10:00:03.000Z".to_owned(),
            raw_read_line: "R2_AFTER_REFRESH".to_owned(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    fwd.recv_message().await.unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut got_reader2 = false;
    while tokio::time::Instant::now() < deadline {
        let msg = match tokio::time::timeout(Duration::from_millis(300), rcv.recv_message()).await {
            Ok(Ok(msg)) => msg,
            Ok(Err(e)) => panic!("recv error: {}", e),
            Err(_) => continue,
        };
        match msg {
            WsMessage::ReceiverEventBatch(batch) => {
                for event in batch.events {
                    assert_ne!(event.raw_read_line, "R1_AFTER_REFRESH");
                    if event.raw_read_line == "R2_AFTER_REFRESH" {
                        got_reader2 = true;
                    }
                }
                if got_reader2 {
                    break;
                }
            }
            WsMessage::ReceiverSelectionApplied(_) | WsMessage::Heartbeat(_) => {}
            other => panic!(
                "unexpected message while waiting for live event: {:?}",
                other
            ),
        }
    }

    assert!(
        got_reader2,
        "expected live events from refreshed stream mapping"
    );
}

#[tokio::test]
async fn test_race_selection_applied_warns_when_current_scope_resolves_no_streams() {
    let (pool, addr) = start_server().await;
    let (_fwd, _fwd_session) = connect_forwarder(&pool, addr).await;
    insert_token(&pool, "rcv-race", "receiver", b"rcv-race-token").await;

    let race_id = insert_race(&pool, "warn-empty").await;
    map_epoch(&pool, "10.30.0.1:10000", 1, &race_id).await;
    sqlx::query(
        "UPDATE streams SET stream_epoch = 2 WHERE forwarder_id = 'fwd-race' AND reader_ip = '10.30.0.1:10000'",
    )
    .execute(&pool)
    .await
    .unwrap();

    let url = format!("ws://{}/ws/v1.1/receivers", addr);
    let mut rcv = MockWsClient::connect_with_token(&url, "rcv-race-token")
        .await
        .unwrap();

    rcv.send_message(&WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-race".to_owned(),
        selection: ReceiverSelection::Race {
            race_id,
            epoch_scope: EpochScope::Current,
        },
        replay_policy: ReplayPolicy::LiveOnly,
        replay_targets: None,
    }))
    .await
    .unwrap();

    let _ = rcv.recv_message().await.unwrap();
    match rcv.recv_message().await.unwrap() {
        WsMessage::ReceiverSelectionApplied(applied) => {
            assert_eq!(applied.resolved_target_count, 0);
            assert!(
                applied
                    .warnings
                    .iter()
                    .any(|warning| warning.contains("mismatch") || warning.contains("no streams")),
                "expected mismatch-oriented warning for empty current-scope resolution: {:?}",
                applied.warnings
            );
        }
        other => panic!("expected selection_applied, got {:?}", other),
    }
}
