//! Integration tests for receiver v1.2 mode behavior.
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

    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
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
    addr: std::net::SocketAddr,
    token: &str,
    forwarder_id: &str,
    reader_ip: &str,
) -> MockWsClient {
    let fwd_url = format!("ws://{addr}/ws/v1/forwarders");
    let mut fwd = MockWsClient::connect_with_token(&fwd_url, token)
        .await
        .unwrap();
    fwd.send_message(&WsMessage::ForwarderHello(ForwarderHello {
        forwarder_id: forwarder_id.to_owned(),
        reader_ips: vec![reader_ip.to_owned()],
        display_name: None,
    }))
    .await
    .unwrap();
    fwd
}

async fn connect_receiver_v12(
    addr: std::net::SocketAddr,
    token: &str,
    hello: ReceiverHelloV12,
) -> (MockWsClient, String) {
    let rcv_url = format!("ws://{addr}/ws/v1.2/receivers");
    let mut rcv = MockWsClient::connect_with_token(&rcv_url, token)
        .await
        .unwrap();
    rcv.send_message(&WsMessage::ReceiverHelloV12(hello))
        .await
        .unwrap();

    let session_id = match rcv.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected initial heartbeat, got {other:?}"),
    };

    (rcv, session_id)
}

async fn send_forwarder_event(
    fwd: &mut MockWsClient,
    session_id: &str,
    forwarder_id: &str,
    reader_ip: &str,
    stream_epoch: u64,
    seq: u64,
    raw_line: &str,
) {
    fwd.send_message(&WsMessage::ForwarderEventBatch(ForwarderEventBatch {
        session_id: session_id.to_owned(),
        batch_id: format!("b-{stream_epoch}-{seq}"),
        events: vec![ReadEvent {
            forwarder_id: forwarder_id.to_owned(),
            reader_ip: reader_ip.to_owned(),
            stream_epoch,
            seq,
            reader_timestamp: "2026-02-25T12:00:00.000Z".to_owned(),
            raw_frame: raw_line.as_bytes().to_vec(),
            read_type: "RAW".to_owned(),
        }],
    }))
    .await
    .unwrap();
    let _ = fwd.recv_message().await.unwrap();
}

async fn wait_for_stream_id(
    pool: &sqlx::PgPool,
    forwarder_id: &str,
    reader_ip: &str,
) -> uuid::Uuid {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let row =
            sqlx::query("SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2")
                .bind(forwarder_id)
                .bind(reader_ip)
                .fetch_optional(pool)
                .await
                .unwrap();

        if let Some(row) = row {
            return row.get("stream_id");
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "stream not found in time"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn recv_first_event_batch(
    client: &mut MockWsClient,
    timeout: Duration,
) -> ReceiverEventBatch {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timeout waiting for receiver event batch"
        );
        match tokio::time::timeout(remaining, client.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => return batch,
            Ok(Ok(WsMessage::ReceiverModeApplied(_))) => continue,
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            Ok(Ok(other)) => panic!("expected receiver event batch, got {other:?}"),
            Ok(Err(err)) => panic!("recv error: {err}"),
            Err(_) => panic!("timeout waiting for receiver event batch"),
        }
    }
}

async fn recv_batch_with_event_seq(
    client: &mut MockWsClient,
    target_seq: u64,
    timeout: Duration,
) -> ReceiverEventBatch {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        assert!(
            !remaining.is_zero(),
            "timeout waiting for receiver batch with seq={target_seq}"
        );
        match tokio::time::timeout(remaining, client.recv_message()).await {
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                if batch.events.iter().any(|event| event.seq == target_seq) {
                    return batch;
                }
            }
            Ok(Ok(WsMessage::ReceiverModeApplied(_))) => continue,
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            Ok(Ok(other)) => panic!("expected receiver event batch, got {other:?}"),
            Ok(Err(err)) => panic!("recv error: {err}"),
            Err(_) => panic!("timeout waiting for receiver event batch"),
        }
    }
}

async fn collect_event_lines_until_idle(
    client: &mut MockWsClient,
    max_total: Duration,
    idle_quiet_period: Duration,
    require_event_batch: bool,
) -> (Vec<String>, bool) {
    let overall_deadline = tokio::time::Instant::now() + max_total;
    let mut seen = Vec::new();
    let mut saw_mode_applied = false;
    let mut saw_event_batch = false;
    let mut last_event_at = tokio::time::Instant::now();

    loop {
        let now = tokio::time::Instant::now();
        if now >= overall_deadline {
            break;
        }

        if saw_event_batch && now.duration_since(last_event_at) >= idle_quiet_period {
            break;
        }

        let remaining_total = overall_deadline.saturating_duration_since(now);
        let wait_for = if saw_event_batch {
            let until_idle = idle_quiet_period.saturating_sub(now.duration_since(last_event_at));
            until_idle.min(remaining_total)
        } else {
            remaining_total
        };
        assert!(
            !wait_for.is_zero(),
            "zero wait while collecting receiver events"
        );

        match tokio::time::timeout(wait_for, client.recv_message()).await {
            Err(_) => {
                if should_stop_collecting_on_timeout(
                    require_event_batch,
                    saw_event_batch,
                    saw_mode_applied,
                ) {
                    break;
                }
                panic!("timeout waiting for receiver event batch");
            }
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                for event in batch.events {
                    seen.push(String::from_utf8_lossy(&event.raw_frame).to_string());
                }
                saw_event_batch = true;
                last_event_at = tokio::time::Instant::now();
            }
            Ok(Ok(WsMessage::ReceiverModeApplied(_))) => {
                saw_mode_applied = true;
            }
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            Ok(Ok(other)) => {
                panic!("unexpected message while collecting receiver events: {other:?}")
            }
            Ok(Err(err)) => panic!("recv error: {err}"),
        }
    }

    assert!(
        !require_event_batch || saw_event_batch,
        "timeout waiting for receiver event batch"
    );

    (seen, saw_mode_applied)
}

fn should_stop_collecting_on_timeout(
    require_event_batch: bool,
    saw_event_batch: bool,
    _saw_mode_applied: bool,
) -> bool {
    saw_event_batch || !require_event_batch
}

#[test]
fn collect_timeout_stops_when_event_batch_already_seen() {
    assert!(should_stop_collecting_on_timeout(true, true, false));
}

#[test]
fn collect_timeout_panics_when_event_batch_required_and_not_seen() {
    assert!(!should_stop_collecting_on_timeout(true, false, false));
}

#[test]
fn collect_timeout_stops_for_optional_collection_even_if_quiet() {
    assert!(should_stop_collecting_on_timeout(false, false, false));
}

#[tokio::test]
async fn receiver_v12_live_uses_persisted_then_earliest_then_current_precedence() {
    let (pool, addr) = start_server().await;
    insert_token(&pool, "fwd-live", "forwarder", b"fwd-live-token").await;
    insert_token(&pool, "rcv-live", "receiver", b"rcv-live-token").await;

    let mut fwd = connect_forwarder(addr, "fwd-live-token", "fwd-live", "10.10.0.1:10000").await;
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {other:?}"),
    };

    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-live",
        "10.10.0.1:10000",
        4,
        1,
        "LIVE_E4_S1",
    )
    .await;
    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-live",
        "10.10.0.1:10000",
        5,
        1,
        "LIVE_E5_S1",
    )
    .await;

    let hello = ReceiverHelloV12 {
        receiver_id: "rcv-live".to_owned(),
        mode: ReceiverMode::Live {
            streams: vec![StreamRef {
                forwarder_id: "fwd-live".to_owned(),
                reader_ip: "10.10.0.1:10000".to_owned(),
            }],
            earliest_epochs: vec![EarliestEpochOverride {
                forwarder_id: "fwd-live".to_owned(),
                reader_ip: "10.10.0.1:10000".to_owned(),
                earliest_epoch: 4,
            }],
        },
        // This stale client-side resume must NOT override live start semantics.
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-live".to_owned(),
            reader_ip: "10.10.0.1:10000".to_owned(),
            stream_epoch: 99,
            last_seq: 999,
        }],
    };

    let (mut rcv, _session_id) = connect_receiver_v12(addr, "rcv-live-token", hello).await;

    let replay = recv_first_event_batch(&mut rcv, Duration::from_secs(5)).await;

    let seqs: Vec<(u64, u64)> = replay
        .events
        .iter()
        .map(|event| (event.stream_epoch, event.seq))
        .collect();
    assert_eq!(seqs, vec![(4, 1), (5, 1)]);
}

#[tokio::test]
async fn receiver_v12_receiver_subscribe_replays_from_persisted_cursor_without_reconnect() {
    let (pool, addr) = start_server().await;
    insert_token(
        &pool,
        "fwd-mid-subscribe",
        "forwarder",
        b"fwd-mid-subscribe-token",
    )
    .await;
    insert_token(
        &pool,
        "rcv-mid-subscribe",
        "receiver",
        b"rcv-mid-subscribe-token",
    )
    .await;

    let mut fwd = connect_forwarder(
        addr,
        "fwd-mid-subscribe-token",
        "fwd-mid-subscribe",
        "10.42.0.1:10000",
    )
    .await;
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {other:?}"),
    };

    let hello = ReceiverHelloV12 {
        receiver_id: "rcv-mid-subscribe".to_owned(),
        mode: ReceiverMode::Live {
            streams: vec![StreamRef {
                forwarder_id: "fwd-mid-subscribe".to_owned(),
                reader_ip: "10.42.0.1:10000".to_owned(),
            }],
            earliest_epochs: vec![],
        },
        resume: vec![],
    };

    let (mut rcv, rcv_session_id) =
        connect_receiver_v12(addr, "rcv-mid-subscribe-token", hello).await;

    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-mid-subscribe",
        "10.42.0.1:10000",
        1,
        1,
        "MID_SUB_E1_S1",
    )
    .await;
    let first = recv_batch_with_event_seq(&mut rcv, 1, Duration::from_secs(5)).await;
    let first_event = first.events.first().expect("first event missing");
    rcv.send_message(&WsMessage::ReceiverAck(ReceiverAck {
        session_id: rcv_session_id.clone(),
        entries: vec![AckEntry {
            forwarder_id: first_event.forwarder_id.clone(),
            reader_ip: first_event.reader_ip.clone(),
            stream_epoch: first_event.stream_epoch,
            last_seq: first_event.seq,
        }],
    }))
    .await
    .unwrap();

    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-mid-subscribe",
        "10.42.0.1:10000",
        1,
        2,
        "MID_SUB_E1_S2",
    )
    .await;
    let _ = recv_batch_with_event_seq(&mut rcv, 2, Duration::from_secs(5)).await;

    rcv.send_message(&WsMessage::ReceiverSubscribe(ReceiverSubscribe {
        session_id: rcv_session_id.clone(),
        streams: vec![StreamRef {
            forwarder_id: "fwd-mid-subscribe".to_owned(),
            reader_ip: "10.42.0.1:10000".to_owned(),
        }],
    }))
    .await
    .unwrap();

    let replayed = recv_batch_with_event_seq(&mut rcv, 2, Duration::from_secs(5)).await;
    assert!(
        replayed
            .events
            .iter()
            .any(|event| event.seq == 2 && event.raw_frame == b"MID_SUB_E1_S2".to_vec()),
        "receiver_subscribe should replay from persisted cursor without reconnect"
    );
}

#[tokio::test]
async fn receiver_v12_targeted_replay_stays_open_but_does_not_stream_live_after_replay() {
    let (pool, addr) = start_server().await;
    insert_token(&pool, "fwd-tr", "forwarder", b"fwd-tr-token").await;
    insert_token(&pool, "rcv-tr", "receiver", b"rcv-tr-token").await;

    let mut fwd = connect_forwarder(addr, "fwd-tr-token", "fwd-tr", "10.11.0.1:10000").await;
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {other:?}"),
    };

    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-tr",
        "10.11.0.1:10000",
        2,
        1,
        "TR_E2_S1",
    )
    .await;

    let hello = ReceiverHelloV12 {
        receiver_id: "rcv-tr".to_owned(),
        mode: ReceiverMode::TargetedReplay {
            targets: vec![ReplayTarget {
                forwarder_id: "fwd-tr".to_owned(),
                reader_ip: "10.11.0.1:10000".to_owned(),
                stream_epoch: 2,
                from_seq: 1,
            }],
        },
        resume: vec![],
    };

    let (mut rcv, _session_id) = connect_receiver_v12(addr, "rcv-tr-token", hello).await;

    let replay = recv_first_event_batch(&mut rcv, Duration::from_secs(5)).await;
    assert_eq!(replay.events.len(), 1);
    assert_eq!(replay.events[0].raw_frame, b"TR_E2_S1".to_vec());

    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-tr",
        "10.11.0.1:10000",
        2,
        2,
        "TR_LIVE_SHOULD_NOT_STREAM",
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_millis(400);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, rcv.recv_message()).await {
            Err(_) => break,
            Ok(Ok(WsMessage::Heartbeat(_))) => continue,
            Ok(Ok(WsMessage::ReceiverModeApplied(_))) => continue,
            Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => {
                panic!(
                    "targeted replay mode must not stream live events after replay; got {batch:?}"
                )
            }
            Ok(Ok(other)) => panic!("unexpected message after replay: {other:?}"),
            Ok(Err(err)) => panic!("recv error: {err}"),
        }
    }
}

#[tokio::test]
async fn receiver_v12_targeted_replay_ack_does_not_persist_receiver_cursor() {
    let (pool, addr) = start_server().await;
    insert_token(&pool, "fwd-tr-ack", "forwarder", b"fwd-tr-ack-token").await;
    insert_token(&pool, "rcv-tr-ack", "receiver", b"rcv-tr-ack-token").await;

    let mut fwd =
        connect_forwarder(addr, "fwd-tr-ack-token", "fwd-tr-ack", "10.12.0.1:10000").await;
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {other:?}"),
    };

    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-tr-ack",
        "10.12.0.1:10000",
        3,
        5,
        "TR_ACK_E3_S5",
    )
    .await;

    let hello = ReceiverHelloV12 {
        receiver_id: "rcv-tr-ack".to_owned(),
        mode: ReceiverMode::TargetedReplay {
            targets: vec![ReplayTarget {
                forwarder_id: "fwd-tr-ack".to_owned(),
                reader_ip: "10.12.0.1:10000".to_owned(),
                stream_epoch: 3,
                from_seq: 5,
            }],
        },
        resume: vec![],
    };

    let (mut rcv, session_id) = connect_receiver_v12(addr, "rcv-tr-ack-token", hello).await;

    let replay = match tokio::time::timeout(Duration::from_secs(5), rcv.recv_message()).await {
        Ok(Ok(WsMessage::ReceiverEventBatch(batch))) => batch,
        Ok(Ok(other)) => panic!("expected replay batch, got {other:?}"),
        Ok(Err(err)) => panic!("recv error: {err}"),
        Err(_) => panic!("timeout waiting for replay"),
    };

    let event = replay.events.first().expect("one replayed event");
    rcv.send_message(&WsMessage::ReceiverAck(ReceiverAck {
        session_id,
        entries: vec![AckEntry {
            forwarder_id: event.forwarder_id.clone(),
            reader_ip: event.reader_ip.clone(),
            stream_epoch: event.stream_epoch,
            last_seq: event.seq,
        }],
    }))
    .await
    .unwrap();

    tokio::time::sleep(Duration::from_millis(200)).await;
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM receiver_cursors WHERE receiver_id = 'rcv-tr-ack'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "targeted replay acks must not persist cursors");
}

#[tokio::test]
async fn receiver_v12_race_includes_non_current_epoch_mapping_and_replays_on_new_streams() {
    let (pool, addr) = start_server().await;
    insert_token(&pool, "fwd-race-a", "forwarder", b"fwd-race-a-token").await;
    insert_token(&pool, "fwd-race-b", "forwarder", b"fwd-race-b-token").await;
    insert_token(&pool, "rcv-race", "receiver", b"rcv-race-token").await;

    let race_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO races (race_id, name) VALUES ($1, $2)")
        .bind(race_id)
        .bind("Race Test")
        .execute(&pool)
        .await
        .unwrap();

    let mut fwd_a =
        connect_forwarder(addr, "fwd-race-a-token", "fwd-race-a", "10.13.0.1:10000").await;
    let fwd_a_session = match fwd_a.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {other:?}"),
    };
    let mut fwd_b =
        connect_forwarder(addr, "fwd-race-b-token", "fwd-race-b", "10.13.0.2:10000").await;
    let fwd_b_session = match fwd_b.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {other:?}"),
    };

    send_forwarder_event(
        &mut fwd_a,
        &fwd_a_session,
        "fwd-race-a",
        "10.13.0.1:10000",
        1,
        1,
        "RACE_A_E1_S1",
    )
    .await;
    send_forwarder_event(
        &mut fwd_a,
        &fwd_a_session,
        "fwd-race-a",
        "10.13.0.1:10000",
        2,
        1,
        "RACE_A_E2_S1",
    )
    .await;
    send_forwarder_event(
        &mut fwd_a,
        &fwd_a_session,
        "fwd-race-a",
        "10.13.0.1:10000",
        3,
        1,
        "RACE_A_E3_S1",
    )
    .await;

    let stream_a_id = wait_for_stream_id(&pool, "fwd-race-a", "10.13.0.1:10000").await;
    let stream_b_id = wait_for_stream_id(&pool, "fwd-race-b", "10.13.0.2:10000").await;

    // Map only epoch 1 for stream A, while current stream epoch is 3.
    sqlx::query(
        "INSERT INTO stream_epoch_races (stream_id, stream_epoch, race_id) VALUES ($1, $2, $3)",
    )
    .bind(stream_a_id)
    .bind(1_i64)
    .bind(race_id)
    .execute(&pool)
    .await
    .unwrap();

    let hello = ReceiverHelloV12 {
        receiver_id: "rcv-race".to_owned(),
        mode: ReceiverMode::Race {
            race_id: race_id.to_string(),
        },
        resume: vec![],
    };
    let (mut rcv, _session_id) = connect_receiver_v12(addr, "rcv-race-token", hello).await;

    // Must resolve with current_only=false and replay existing stream A data.
    let first_replay = recv_first_event_batch(&mut rcv, Duration::from_secs(5)).await;
    assert!(
        first_replay
            .events
            .iter()
            .any(|event| event.raw_frame == b"RACE_A_E1_S1".to_vec()),
        "race mode must include non-current mapped epoch stream"
    );

    // Add backlog for stream B, then add race mapping after receiver already connected.
    send_forwarder_event(
        &mut fwd_b,
        &fwd_b_session,
        "fwd-race-b",
        "10.13.0.2:10000",
        1,
        1,
        "RACE_B_BACKLOG_E1_S1",
    )
    .await;

    sqlx::query(
        "INSERT INTO stream_epoch_races (stream_id, stream_epoch, race_id) VALUES ($1, $2, $3)",
    )
    .bind(stream_b_id)
    .bind(1_i64)
    .bind(race_id)
    .execute(&pool)
    .await
    .unwrap();

    // Forward-only refresh must add the new stream without starting at tail.
    let refreshed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match rcv.recv_message().await {
                Ok(WsMessage::ReceiverEventBatch(batch))
                    if batch
                        .events
                        .iter()
                        .any(|event| event.raw_frame == b"RACE_B_BACKLOG_E1_S1".to_vec()) =>
                {
                    break batch;
                }
                Ok(WsMessage::ReceiverEventBatch(_)) => continue,
                Ok(WsMessage::ReceiverModeApplied(_)) => continue,
                Ok(WsMessage::Heartbeat(_)) => continue,
                Ok(other) => {
                    panic!("unexpected message while waiting for race refresh replay: {other:?}")
                }
                Err(err) => panic!("recv error: {err}"),
            }
        }
    })
    .await;

    assert!(
        refreshed.is_ok(),
        "new race streams must replay backlog after refresh"
    );
}

#[tokio::test]
async fn receiver_v12_live_stale_resume_without_prior_events_still_streams_live_data() {
    let (pool, addr) = start_server().await;
    insert_token(
        &pool,
        "fwd-live-fallback",
        "forwarder",
        b"fwd-live-fallback-token",
    )
    .await;
    insert_token(
        &pool,
        "rcv-live-fallback",
        "receiver",
        b"rcv-live-fallback-token",
    )
    .await;

    let mut fwd = connect_forwarder(
        addr,
        "fwd-live-fallback-token",
        "fwd-live-fallback",
        "10.14.0.1:10000",
    )
    .await;
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {other:?}"),
    };

    let hello = ReceiverHelloV12 {
        receiver_id: "rcv-live-fallback".to_owned(),
        mode: ReceiverMode::Live {
            streams: vec![StreamRef {
                forwarder_id: "fwd-live-fallback".to_owned(),
                reader_ip: "10.14.0.1:10000".to_owned(),
            }],
            earliest_epochs: vec![],
        },
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-live-fallback".to_owned(),
            reader_ip: "10.14.0.1:10000".to_owned(),
            stream_epoch: 99,
            last_seq: 999,
        }],
    };
    let (mut rcv, _session_id) = connect_receiver_v12(addr, "rcv-live-fallback-token", hello).await;

    // No prior events exist. A stale cursor must not prevent receiving subsequent live data.
    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-live-fallback",
        "10.14.0.1:10000",
        1,
        1,
        "LIVE_FALLBACK_E1_S1",
    )
    .await;

    let batch = recv_first_event_batch(&mut rcv, Duration::from_secs(5)).await;
    assert!(
        batch
            .events
            .iter()
            .any(|event| event.raw_frame == b"LIVE_FALLBACK_E1_S1".to_vec())
    );
}

#[tokio::test]
async fn receiver_v12_race_stale_resume_does_not_filter_replayed_older_epoch_data() {
    let (pool, addr) = start_server().await;
    insert_token(
        &pool,
        "fwd-race-stale",
        "forwarder",
        b"fwd-race-stale-token",
    )
    .await;
    insert_token(&pool, "rcv-race-stale", "receiver", b"rcv-race-stale-token").await;

    let race_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO races (race_id, name) VALUES ($1, $2)")
        .bind(race_id)
        .bind("Race Stale Cursor")
        .execute(&pool)
        .await
        .unwrap();

    let mut fwd = connect_forwarder(
        addr,
        "fwd-race-stale-token",
        "fwd-race-stale",
        "10.15.0.1:10000",
    )
    .await;
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {other:?}"),
    };

    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-race-stale",
        "10.15.0.1:10000",
        1,
        1,
        "RACE_STALE_E1_S1",
    )
    .await;
    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-race-stale",
        "10.15.0.1:10000",
        2,
        1,
        "RACE_STALE_E2_S1",
    )
    .await;

    let stream_id = wait_for_stream_id(&pool, "fwd-race-stale", "10.15.0.1:10000").await;
    sqlx::query(
        "INSERT INTO stream_epoch_races (stream_id, stream_epoch, race_id) VALUES ($1, $2, $3)",
    )
    .bind(stream_id)
    .bind(1_i64)
    .bind(race_id)
    .execute(&pool)
    .await
    .unwrap();

    let hello = ReceiverHelloV12 {
        receiver_id: "rcv-race-stale".to_owned(),
        mode: ReceiverMode::Race {
            race_id: race_id.to_string(),
        },
        // Stale/high cursor must not suppress replay of race-mapped older epoch.
        resume: vec![ResumeCursor {
            forwarder_id: "fwd-race-stale".to_owned(),
            reader_ip: "10.15.0.1:10000".to_owned(),
            stream_epoch: 2,
            last_seq: 999,
        }],
    };
    let (mut rcv, _session_id) = connect_receiver_v12(addr, "rcv-race-stale-token", hello).await;

    let (replayed_lines, _saw_mode_applied) = collect_event_lines_until_idle(
        &mut rcv,
        Duration::from_secs(5),
        Duration::from_millis(250),
        true,
    )
    .await;
    assert!(
        replayed_lines.iter().any(|line| line == "RACE_STALE_E1_S1"),
        "race replay should include mapped epoch data regardless of stale resume cursor"
    );
    assert!(
        replayed_lines.iter().any(|line| line == "RACE_STALE_E2_S1"),
        "race selection is stream-level in v1.2 and replays same-stream epochs once selected"
    );
}

#[tokio::test]
async fn receiver_v12_targeted_replay_is_snapshot_bounded_when_new_events_arrive_after_mode_apply()
{
    let (pool, addr) = start_server().await;
    insert_token(
        &pool,
        "fwd-tr-snapshot",
        "forwarder",
        b"fwd-tr-snapshot-token",
    )
    .await;
    insert_token(
        &pool,
        "rcv-tr-snapshot",
        "receiver",
        b"rcv-tr-snapshot-token",
    )
    .await;

    let mut fwd = connect_forwarder(
        addr,
        "fwd-tr-snapshot-token",
        "fwd-tr-snapshot",
        "10.16.0.1:10000",
    )
    .await;
    let fwd_session = match fwd.recv_message().await.unwrap() {
        WsMessage::Heartbeat(h) => h.session_id,
        other => panic!("expected heartbeat, got {other:?}"),
    };

    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-tr-snapshot",
        "10.16.0.1:10000",
        7,
        1,
        "TR_SNAP_E7_S1",
    )
    .await;
    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-tr-snapshot",
        "10.16.0.1:10000",
        7,
        2,
        "TR_SNAP_E7_S2",
    )
    .await;

    let hello = ReceiverHelloV12 {
        receiver_id: "rcv-tr-snapshot".to_owned(),
        mode: ReceiverMode::TargetedReplay {
            targets: vec![ReplayTarget {
                forwarder_id: "fwd-tr-snapshot".to_owned(),
                reader_ip: "10.16.0.1:10000".to_owned(),
                stream_epoch: 7,
                from_seq: 1,
            }],
        },
        resume: vec![],
    };
    let (mut rcv, _session_id) = connect_receiver_v12(addr, "rcv-tr-snapshot-token", hello).await;

    let (mut seen, mut saw_mode_applied) = collect_event_lines_until_idle(
        &mut rcv,
        Duration::from_secs(5),
        Duration::from_millis(250),
        false,
    )
    .await;
    assert!(
        !seen.is_empty() || saw_mode_applied,
        "expected replay batch or ReceiverModeApplied"
    );

    send_forwarder_event(
        &mut fwd,
        &fwd_session,
        "fwd-tr-snapshot",
        "10.16.0.1:10000",
        7,
        3,
        "TR_SNAP_E7_S3_AFTER_APPLY",
    )
    .await;

    let (post_apply_seen, post_apply_saw_mode_applied) = collect_event_lines_until_idle(
        &mut rcv,
        Duration::from_secs(5),
        Duration::from_millis(300),
        false,
    )
    .await;
    seen.extend(post_apply_seen);
    saw_mode_applied |= post_apply_saw_mode_applied;

    assert!(
        saw_mode_applied,
        "expected ReceiverModeApplied during targeted replay session"
    );

    assert!(
        seen.iter().any(|line| line == "TR_SNAP_E7_S1"),
        "expected snapshot to include seq 1"
    );
    assert!(
        seen.iter().any(|line| line == "TR_SNAP_E7_S2"),
        "expected snapshot to include seq 2"
    );
    assert!(
        !seen.iter().any(|line| line == "TR_SNAP_E7_S3_AFTER_APPLY"),
        "targeted replay snapshot must be bounded and exclude events after mode apply"
    );
}
