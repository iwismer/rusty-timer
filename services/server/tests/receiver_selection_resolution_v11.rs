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
