use crate::{
    auth::validate_token,
    repo::{
        events::{
            fetch_events_after_cursor_through_cursor_limited,
            fetch_events_for_stream_epoch_from_seq_through_cursor_limited, fetch_max_event_cursor,
        },
        receiver_cursors::{fetch_cursor, upsert_cursor},
        stream_epoch_races::list_race_selection_streams,
    },
    state::{AppState, ReceiverSelectionSnapshot, ReceiverSessionProtocol},
    ws_common::{
        extract_token_from_headers, recv_text_with_timeout, send_heartbeat, send_ws_error,
    },
};
use axum::extract::{
    State,
    ws::{Message, WebSocket, WebSocketUpgrade},
};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use rt_protocol::{
    EarliestEpochOverride, ReadEvent, ReceiverAck, ReceiverEventBatch, ReceiverHelloV12,
    ReceiverMode, ReceiverModeApplied, ReplayTarget, StreamRef, WsMessage, error_codes,
};
use sqlx::{Row, types::Uuid as SqlUuid};
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const SESSION_TIMEOUT: Duration = Duration::from_secs(90);
const REPLAY_BATCH_LIMIT: i64 = 500;
const RACE_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

pub async fn ws_receiver_v12_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = extract_token_from_headers(&headers);
    ws.on_upgrade(move |socket| handle_receiver_socket(socket, state, token))
}

struct StreamSub {
    stream_id: Uuid,
    last_epoch: i64,
    last_seq: i64,
    rx: tokio::sync::broadcast::Receiver<ReadEvent>,
}

#[derive(Clone)]
struct ResolvedStreamTarget {
    stream_id: Uuid,
    forwarder_id: String,
    reader_ip: String,
    current_stream_epoch: i64,
}

#[derive(Clone)]
struct TargetedReplaySelection {
    stream_id: Uuid,
    stream_epoch: i64,
    from_seq: i64,
    through_epoch: i64,
    through_seq: i64,
}

enum ActiveMode {
    Live,
    Race {
        race_id: String,
        baseline: RaceBaseline,
    },
    TargetedReplay,
}

struct RaceBaseline {
    max_epochs: HashMap<Uuid, i64>,
}

impl RaceBaseline {
    fn includes(&self, stream_id: Uuid, stream_epoch: i64) -> bool {
        self.max_epochs
            .get(&stream_id)
            .is_some_and(|max_epoch| stream_epoch <= *max_epoch)
    }

    fn record(&mut self, stream_id: Uuid, stream_epoch: i64) -> bool {
        use std::collections::hash_map::Entry;

        match self.max_epochs.entry(stream_id) {
            Entry::Occupied(mut entry) => {
                if stream_epoch > *entry.get() {
                    entry.insert(stream_epoch);
                    true
                } else {
                    false
                }
            }
            Entry::Vacant(entry) => {
                entry.insert(stream_epoch);
                true
            }
        }
    }
}

fn cursor_gt(left_epoch: i64, left_seq: i64, right_epoch: i64, right_seq: i64) -> bool {
    (left_epoch, left_seq) > (right_epoch, right_seq)
}

async fn handle_receiver_socket(mut socket: WebSocket, state: AppState, token: Option<String>) {
    let token_str = match token {
        Some(t) => t,
        None => {
            send_ws_error(
                &mut socket,
                error_codes::INVALID_TOKEN,
                "missing Authorization header",
                false,
            )
            .await;
            return;
        }
    };

    let claims = match validate_token(&state.pool, &token_str).await {
        Some(c) => c,
        None => {
            send_ws_error(
                &mut socket,
                error_codes::INVALID_TOKEN,
                "unknown or revoked token",
                false,
            )
            .await;
            return;
        }
    };

    if claims.device_type != "receiver" {
        send_ws_error(
            &mut socket,
            error_codes::INVALID_TOKEN,
            "token is not for a receiver device",
            false,
        )
        .await;
        return;
    }

    let device_id = claims.device_id.clone();

    let hello = match recv_text_with_timeout(&mut socket, SESSION_TIMEOUT).await {
        Ok(text) => match serde_json::from_str::<WsMessage>(&text) {
            Ok(WsMessage::ReceiverHelloV12(hello)) => hello,
            Ok(_) => {
                send_ws_error(
                    &mut socket,
                    error_codes::PROTOCOL_ERROR,
                    "expected receiver_hello_v12",
                    false,
                )
                .await;
                return;
            }
            Err(e) => {
                send_ws_error(
                    &mut socket,
                    error_codes::PROTOCOL_ERROR,
                    &format!("invalid JSON: {e}"),
                    false,
                )
                .await;
                return;
            }
        },
        Err(()) => {
            send_ws_error(
                &mut socket,
                error_codes::PROTOCOL_ERROR,
                "timeout waiting for receiver_hello_v12",
                false,
            )
            .await;
            return;
        }
    };

    if !hello.receiver_id.is_empty() && hello.receiver_id != device_id {
        send_ws_error(
            &mut socket,
            error_codes::IDENTITY_MISMATCH,
            "hello receiver_id does not match token claims",
            false,
        )
        .await;
        return;
    }

    let session_id = Uuid::new_v4().to_string();
    let selection = ReceiverSelectionSnapshot::Mode {
        mode_summary: mode_summary(&hello.mode),
    };

    state
        .register_receiver_session(
            &session_id,
            &device_id,
            ReceiverSessionProtocol::V12,
            selection,
        )
        .await;

    state.logger.log(format!(
        "receiver {device_id} connected (session {session_id}) [v1.2]"
    ));

    let session_id_for_cleanup = session_id.clone();
    async {
        if !send_heartbeat(&mut socket, &session_id, &device_id).await {
            return;
        }

        let mut subscriptions: Vec<StreamSub> = Vec::new();
        let mut active_mode = match apply_mode(
            &mut socket,
            &state,
            &device_id,
            &session_id,
            &hello,
            &mut subscriptions,
        )
        .await
        {
            Ok(mode) => mode,
            Err(e) => {
                error!(device_id = %device_id, error = %e, "error applying receiver mode");
                return;
            }
        };

        let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat_interval.tick().await;
        let mut race_refresh_interval = tokio::time::interval(RACE_REFRESH_INTERVAL);
        race_refresh_interval.tick().await;

        loop {
            let mut events_to_send: Vec<ReadEvent> = Vec::new();
            for sub in &mut subscriptions {
                loop {
                    match sub.rx.try_recv() {
                        Ok(event) => {
                            let Ok(event_epoch) = i64::try_from(event.stream_epoch) else {
                                warn!(
                                    stream_id = %sub.stream_id,
                                    stream_epoch = event.stream_epoch,
                                    "dropping live event with out-of-range stream_epoch"
                                );
                                continue;
                            };
                            let Ok(event_seq) = i64::try_from(event.seq) else {
                                warn!(
                                    stream_id = %sub.stream_id,
                                    seq = event.seq,
                                    "dropping live event with out-of-range seq"
                                );
                                continue;
                            };
                            if !cursor_gt(event_epoch, event_seq, sub.last_epoch, sub.last_seq) {
                                continue;
                            }
                            sub.last_epoch = event_epoch;
                            sub.last_seq = event_seq;
                            events_to_send.push(event);
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                            warn!(
                                device_id = %device_id,
                                stream_id = %sub.stream_id,
                                lagged = n,
                                "receiver lagged, replaying from DB"
                            );
                            if let Err(e) = replay_backlog(&mut socket, &state, &session_id, sub).await
                            {
                                error!(error = %e, "replay failed");
                                return;
                            }
                            break;
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                            warn!(
                                device_id = %device_id,
                                stream_id = %sub.stream_id,
                                "broadcast channel closed"
                            );
                            break;
                        }
                    }
                }
            }

            let mut sent_live_batch = false;
            if !events_to_send.is_empty() {
                let batch = WsMessage::ReceiverEventBatch(ReceiverEventBatch {
                    session_id: session_id.clone(),
                    events: events_to_send,
                });
                if let Ok(json) = serde_json::to_string(&batch)
                    && socket.send(Message::Text(json.into())).await.is_err()
                {
                    break;
                }
                sent_live_batch = true;
            }

            let wait_duration = if sent_live_batch {
                Duration::from_millis(0)
            } else {
                Duration::from_millis(10)
            };

            tokio::select! {
                msg = tokio::time::timeout(wait_duration, socket.recv()) => {
                    match msg {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            match serde_json::from_str::<WsMessage>(&text) {
                                Ok(WsMessage::ReceiverAck(ack)) => {
                                    let persist_cursors = !matches!(active_mode, ActiveMode::TargetedReplay);
                                    if let Err(e) = handle_receiver_ack(&state, &device_id, ack, persist_cursors).await {
                                        error!(device_id = %device_id, error = %e, "error handling receiver ack");
                                    }
                                }
                                Ok(WsMessage::Heartbeat(_)) => {}
                                Ok(WsMessage::ReceiverHelloV12(_)) => {
                                    send_ws_error(
                                        &mut socket,
                                        error_codes::PROTOCOL_ERROR,
                                        "mid-session mode changes are not supported",
                                        false,
                                    )
                                    .await;
                                    break;
                                }
                                Ok(_) => {
                                    send_ws_error(
                                        &mut socket,
                                        error_codes::PROTOCOL_ERROR,
                                        "unexpected message kind",
                                        false,
                                    )
                                    .await;
                                    break;
                                }
                                Err(e) => {
                                    send_ws_error(
                                        &mut socket,
                                        error_codes::PROTOCOL_ERROR,
                                        &format!("invalid JSON: {e}"),
                                        false,
                                    )
                                    .await;
                                    break;
                                }
                            }
                        }
                        Ok(Some(Ok(Message::Ping(data)))) => {
                            let _ = socket.send(Message::Pong(data)).await;
                        }
                        Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                            state.logger.log(format!("receiver {device_id} disconnected"));
                            break;
                        }
                        Err(_) => {}
                        Ok(Some(Err(e))) => {
                            warn!(device_id = %device_id, error = %e, "WS error");
                            break;
                        }
                        Ok(Some(Ok(_))) => {}
                    }
                }
                _ = heartbeat_interval.tick() => {
                    if !send_heartbeat(&mut socket, &session_id, &device_id).await {
                        break;
                    }
                }
                _ = race_refresh_interval.tick() => {
                    let ActiveMode::Race { race_id, baseline } = &mut active_mode else {
                        continue;
                    };

                    match apply_race_refresh_forward_only(
                        &mut socket,
                        &state,
                        &device_id,
                        &session_id,
                        race_id,
                        baseline,
                        &mut subscriptions,
                    )
                    .await {
                        Ok(_changed) => {}
                        Err(e) => {
                            error!(device_id = %device_id, error = %e, "error refreshing race mode");
                            break;
                        }
                    }
                }
            }
        }

        info!(device_id = %device_id, "receiver v1.2 session ended");
        state
            .logger
            .log(format!("receiver {device_id} v1.2 session ended"));
    }
    .await;

    state
        .unregister_receiver_session(&session_id_for_cleanup)
        .await;
}

fn mode_summary(mode: &ReceiverMode) -> String {
    match mode {
        ReceiverMode::Live { streams, .. } => format!("live ({} streams)", streams.len()),
        ReceiverMode::Race { race_id } => format!("race ({race_id})"),
        ReceiverMode::TargetedReplay { targets } => {
            format!("targeted_replay ({} targets)", targets.len())
        }
    }
}

async fn apply_mode(
    socket: &mut WebSocket,
    state: &AppState,
    device_id: &str,
    session_id: &str,
    hello: &ReceiverHelloV12,
    subscriptions: &mut Vec<StreamSub>,
) -> Result<ActiveMode, Box<dyn std::error::Error + Send + Sync>> {
    match &hello.mode {
        ReceiverMode::Live {
            streams,
            earliest_epochs,
        } => {
            let (targets, mut warnings) = resolve_live_targets(state, streams).await?;
            let earliest_map = earliest_epoch_map(earliest_epochs);

            let mut next_subscriptions = Vec::with_capacity(targets.len());
            for target in &targets {
                let start_cursor = compute_live_start_cursor(
                    state,
                    device_id,
                    target,
                    earliest_map
                        .get(&(target.forwarder_id.clone(), target.reader_ip.clone()))
                        .copied(),
                )
                .await?;
                next_subscriptions
                    .push(subscribe_by_stream_id(state, target.stream_id, start_cursor).await);
            }

            for sub in &mut next_subscriptions {
                replay_backlog(socket, state, session_id, sub).await?;
            }

            let applied = WsMessage::ReceiverModeApplied(ReceiverModeApplied {
                mode_summary: mode_summary(&hello.mode),
                resolved_stream_count: next_subscriptions.len(),
                warnings: std::mem::take(&mut warnings),
            });
            socket
                .send(Message::Text(serde_json::to_string(&applied)?.into()))
                .await?;

            *subscriptions = next_subscriptions;
            let _ = state
                .update_receiver_session_selection(
                    session_id,
                    ReceiverSelectionSnapshot::Mode {
                        mode_summary: mode_summary(&hello.mode),
                    },
                )
                .await;
            Ok(ActiveMode::Live)
        }
        ReceiverMode::Race { race_id } => {
            let (mut next_subscriptions, baseline) =
                apply_race_mode_forward_only(state, device_id, race_id).await?;
            for sub in &mut next_subscriptions {
                replay_backlog(socket, state, session_id, sub).await?;
            }

            let applied = WsMessage::ReceiverModeApplied(ReceiverModeApplied {
                mode_summary: mode_summary(&hello.mode),
                resolved_stream_count: next_subscriptions.len(),
                warnings: Vec::new(),
            });
            socket
                .send(Message::Text(serde_json::to_string(&applied)?.into()))
                .await?;

            *subscriptions = next_subscriptions;
            let _ = state
                .update_receiver_session_selection(
                    session_id,
                    ReceiverSelectionSnapshot::Mode {
                        mode_summary: mode_summary(&hello.mode),
                    },
                )
                .await;
            Ok(ActiveMode::Race {
                race_id: race_id.clone(),
                baseline,
            })
        }
        ReceiverMode::TargetedReplay { targets } => {
            let (resolved_targets, mut warnings) =
                resolve_targeted_replay_targets(state, targets).await?;

            let mut transient_subscriptions =
                unique_targeted_subscriptions(state, &resolved_targets).await?;
            replay_targeted_backlog(
                socket,
                state,
                session_id,
                &resolved_targets,
                &mut transient_subscriptions,
            )
            .await?;

            let applied = WsMessage::ReceiverModeApplied(ReceiverModeApplied {
                mode_summary: mode_summary(&hello.mode),
                resolved_stream_count: transient_subscriptions.len(),
                warnings: std::mem::take(&mut warnings),
            });
            socket
                .send(Message::Text(serde_json::to_string(&applied)?.into()))
                .await?;

            // Targeted replay is a one-shot mode. Keep the connection open for
            // heartbeat/acks only, but do not keep any live stream subscriptions.
            subscriptions.clear();
            let _ = state
                .update_receiver_session_selection(
                    session_id,
                    ReceiverSelectionSnapshot::Mode {
                        mode_summary: mode_summary(&hello.mode),
                    },
                )
                .await;
            Ok(ActiveMode::TargetedReplay)
        }
    }
}

async fn apply_race_mode_forward_only(
    state: &AppState,
    device_id: &str,
    race_id: &str,
) -> Result<(Vec<StreamSub>, RaceBaseline), Box<dyn std::error::Error + Send + Sync>> {
    let (targets, baseline) = resolve_race_targets(state, race_id).await?;
    let mut subscriptions = Vec::with_capacity(targets.len());
    for target in targets {
        let start_cursor = fetch_cursor(&state.pool, device_id, target.stream_id)
            .await?
            .unwrap_or((1, 0));
        subscriptions.push(subscribe_by_stream_id(state, target.stream_id, start_cursor).await);
    }
    Ok((subscriptions, baseline))
}

async fn apply_race_refresh_forward_only(
    socket: &mut WebSocket,
    state: &AppState,
    device_id: &str,
    session_id: &str,
    race_id: &str,
    baseline: &mut RaceBaseline,
    subscriptions: &mut Vec<StreamSub>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let (targets, _) = resolve_race_targets(state, race_id).await?;
    if !race_refresh_needed(&targets, subscriptions, baseline) {
        return Ok(false);
    }

    let (mut next_subscriptions, new_targets) =
        plan_race_refresh_subscriptions(targets, std::mem::take(subscriptions), baseline);

    for target in new_targets {
        let start_cursor = fetch_cursor(&state.pool, device_id, target.stream_id)
            .await?
            .unwrap_or((1, 0));
        let mut new_sub = subscribe_by_stream_id(state, target.stream_id, start_cursor).await;
        // New subscriptions (including remove/re-add) must replay from persisted cursor.
        replay_backlog(socket, state, session_id, &mut new_sub).await?;
        next_subscriptions.push(new_sub);
    }

    *subscriptions = next_subscriptions;

    let applied = WsMessage::ReceiverModeApplied(ReceiverModeApplied {
        mode_summary: format!("race ({race_id})"),
        resolved_stream_count: subscriptions.len(),
        warnings: Vec::new(),
    });
    socket
        .send(Message::Text(serde_json::to_string(&applied)?.into()))
        .await?;
    let _ = state
        .update_receiver_session_selection(
            session_id,
            ReceiverSelectionSnapshot::Mode {
                mode_summary: format!("race ({race_id})"),
            },
        )
        .await;

    Ok(true)
}

fn plan_race_refresh_subscriptions(
    targets: Vec<ResolvedStreamTarget>,
    subscriptions: Vec<StreamSub>,
    baseline: &mut RaceBaseline,
) -> (Vec<StreamSub>, Vec<ResolvedStreamTarget>) {
    let mut existing_subscriptions: HashMap<Uuid, StreamSub> = subscriptions
        .into_iter()
        .map(|sub| (sub.stream_id, sub))
        .collect();
    let mut next_subscriptions = Vec::with_capacity(targets.len());
    let mut new_targets = Vec::new();

    for target in targets {
        if let Some(existing) = existing_subscriptions.remove(&target.stream_id) {
            next_subscriptions.push(existing);
        } else {
            new_targets.push(target.clone());
        }
        baseline.record(target.stream_id, target.current_stream_epoch);
    }

    (next_subscriptions, new_targets)
}

fn race_refresh_needed(
    targets: &[ResolvedStreamTarget],
    subscriptions: &[StreamSub],
    baseline: &RaceBaseline,
) -> bool {
    let target_stream_ids: HashSet<Uuid> = targets.iter().map(|target| target.stream_id).collect();
    let subscribed_stream_ids: HashSet<Uuid> =
        subscriptions.iter().map(|sub| sub.stream_id).collect();
    if target_stream_ids != subscribed_stream_ids {
        return true;
    }

    targets
        .iter()
        .any(|target| !baseline.includes(target.stream_id, target.current_stream_epoch))
}

fn earliest_epoch_map(overrides: &[EarliestEpochOverride]) -> HashMap<(String, String), i64> {
    let mut map = HashMap::new();
    for override_row in overrides {
        map.insert(
            (
                override_row.forwarder_id.clone(),
                override_row.reader_ip.clone(),
            ),
            override_row.earliest_epoch,
        );
    }
    map
}

async fn compute_live_start_cursor(
    state: &AppState,
    device_id: &str,
    target: &ResolvedStreamTarget,
    earliest_epoch: Option<i64>,
) -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>> {
    // Cursor precedence: persisted > earliest override > current stream epoch.
    let cursor = match fetch_cursor(&state.pool, device_id, target.stream_id).await? {
        Some(persisted) => persisted,
        None => match earliest_epoch {
            Some(earliest) => (earliest, 0),
            None => (target.current_stream_epoch, 0),
        },
    };
    Ok(cursor)
}

async fn resolve_live_targets(
    state: &AppState,
    streams: &[StreamRef],
) -> Result<(Vec<ResolvedStreamTarget>, Vec<String>), Box<dyn std::error::Error + Send + Sync>> {
    let mut dedup = HashSet::new();
    let mut targets = Vec::new();
    let mut warnings = Vec::new();

    for stream in streams {
        if !dedup.insert((stream.forwarder_id.clone(), stream.reader_ip.clone())) {
            continue;
        }

        let row = sqlx::query(
            "SELECT stream_id, stream_epoch FROM streams WHERE forwarder_id = $1 AND reader_ip = $2",
        )
        .bind(&stream.forwarder_id)
        .bind(&stream.reader_ip)
        .fetch_optional(&state.pool)
        .await?;

        if let Some(row) = row {
            targets.push(ResolvedStreamTarget {
                stream_id: row.get("stream_id"),
                forwarder_id: stream.forwarder_id.clone(),
                reader_ip: stream.reader_ip.clone(),
                current_stream_epoch: row.get("stream_epoch"),
            });
        } else {
            warnings.push(format!(
                "stream not found for {}:{}",
                stream.forwarder_id, stream.reader_ip
            ));
        }
    }

    Ok((targets, warnings))
}

async fn resolve_race_targets(
    state: &AppState,
    race_id: &str,
) -> Result<(Vec<ResolvedStreamTarget>, RaceBaseline), Box<dyn std::error::Error + Send + Sync>> {
    let Ok(race_uuid) = SqlUuid::parse_str(race_id) else {
        return Ok((
            Vec::new(),
            RaceBaseline {
                max_epochs: HashMap::new(),
            },
        ));
    };

    let rows = list_race_selection_streams(&state.pool, race_uuid, false).await?;
    let mut targets = Vec::with_capacity(rows.len());
    let mut max_epochs: HashMap<Uuid, i64> = HashMap::with_capacity(rows.len());
    for row in rows {
        max_epochs
            .entry(row.stream_id)
            .and_modify(|max_epoch| {
                if row.stream_epoch > *max_epoch {
                    *max_epoch = row.stream_epoch;
                }
            })
            .or_insert(row.stream_epoch);
        targets.push(ResolvedStreamTarget {
            stream_id: row.stream_id,
            forwarder_id: row.forwarder_id,
            reader_ip: row.reader_ip,
            current_stream_epoch: row.stream_epoch,
        });
    }
    Ok((targets, RaceBaseline { max_epochs }))
}

async fn resolve_targeted_replay_targets(
    state: &AppState,
    targets: &[ReplayTarget],
) -> Result<(Vec<TargetedReplaySelection>, Vec<String>), Box<dyn std::error::Error + Send + Sync>> {
    let mut dedup = HashSet::new();
    let mut resolved = Vec::new();
    let mut warnings = Vec::new();

    for target in targets {
        let from_seq = target.from_seq.max(1);
        if !dedup.insert((
            target.forwarder_id.clone(),
            target.reader_ip.clone(),
            target.stream_epoch,
            from_seq,
        )) {
            continue;
        }

        let row =
            sqlx::query("SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2")
                .bind(&target.forwarder_id)
                .bind(&target.reader_ip)
                .fetch_optional(&state.pool)
                .await?;

        let Some(row) = row else {
            warnings.push(format!(
                "ignored replay_target for unknown stream {}:{}",
                target.forwarder_id, target.reader_ip
            ));
            continue;
        };

        let stream_id: Uuid = row.get("stream_id");
        let Some((through_epoch, through_seq)) =
            fetch_max_event_cursor(&state.pool, stream_id).await?
        else {
            continue;
        };

        if through_epoch < target.stream_epoch
            || (through_epoch == target.stream_epoch && through_seq < from_seq)
        {
            continue;
        }

        resolved.push(TargetedReplaySelection {
            stream_id,
            stream_epoch: target.stream_epoch,
            from_seq,
            through_epoch,
            through_seq,
        });
    }

    Ok((resolved, warnings))
}

async fn unique_targeted_subscriptions(
    state: &AppState,
    targets: &[TargetedReplaySelection],
) -> Result<Vec<StreamSub>, Box<dyn std::error::Error + Send + Sync>> {
    let mut stream_ids = HashSet::new();
    let mut subscriptions = Vec::new();

    for target in targets {
        if !stream_ids.insert(target.stream_id) {
            continue;
        }

        let tail = fetch_max_event_cursor(&state.pool, target.stream_id)
            .await?
            .unwrap_or((1, 0));
        subscriptions.push(subscribe_by_stream_id(state, target.stream_id, tail).await);
    }

    Ok(subscriptions)
}

async fn subscribe_by_stream_id(
    state: &AppState,
    stream_id: Uuid,
    start_cursor: (i64, i64),
) -> StreamSub {
    let tx = state.get_or_create_broadcast(stream_id).await;
    StreamSub {
        stream_id,
        last_epoch: start_cursor.0,
        last_seq: start_cursor.1,
        rx: tx.subscribe(),
    }
}

async fn replay_backlog(
    socket: &mut WebSocket,
    state: &AppState,
    session_id: &str,
    sub: &mut StreamSub,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some((through_epoch, through_seq)) =
        fetch_max_event_cursor(&state.pool, sub.stream_id).await?
    else {
        return Ok(());
    };

    if !cursor_gt(through_epoch, through_seq, sub.last_epoch, sub.last_seq) {
        return Ok(());
    }

    loop {
        let events = fetch_events_after_cursor_through_cursor_limited(
            &state.pool,
            sub.stream_id,
            sub.last_epoch,
            sub.last_seq,
            through_epoch,
            through_seq,
            REPLAY_BATCH_LIMIT,
        )
        .await?;

        if events.is_empty() {
            return Ok(());
        }

        let read_events: Vec<ReadEvent> = events
            .iter()
            .map(|event| ReadEvent {
                forwarder_id: event.forwarder_id.clone(),
                reader_ip: event.reader_ip.clone(),
                stream_epoch: event.stream_epoch as u64,
                seq: event.seq as u64,
                reader_timestamp: event.reader_timestamp.clone().unwrap_or_default(),
                raw_read_line: event.raw_read_line.clone(),
                read_type: event.read_type.clone(),
            })
            .collect();

        if let Some(last) = events.last() {
            sub.last_epoch = last.stream_epoch;
            sub.last_seq = last.seq;
        }

        let batch = WsMessage::ReceiverEventBatch(ReceiverEventBatch {
            session_id: session_id.to_owned(),
            events: read_events,
        });
        socket
            .send(Message::Text(serde_json::to_string(&batch)?.into()))
            .await?;

        if !cursor_gt(through_epoch, through_seq, sub.last_epoch, sub.last_seq)
            || events.len() < REPLAY_BATCH_LIMIT as usize
        {
            return Ok(());
        }
    }
}

async fn replay_targeted_backlog(
    socket: &mut WebSocket,
    state: &AppState,
    session_id: &str,
    targets: &[TargetedReplaySelection],
    subscriptions: &mut [StreamSub],
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for target in targets {
        let mut from_seq = target.from_seq;
        loop {
            let events = fetch_events_for_stream_epoch_from_seq_through_cursor_limited(
                &state.pool,
                target.stream_id,
                target.stream_epoch,
                from_seq,
                target.through_epoch,
                target.through_seq,
                REPLAY_BATCH_LIMIT,
            )
            .await?;

            if events.is_empty() {
                break;
            }

            let read_events: Vec<ReadEvent> = events
                .iter()
                .map(|event| ReadEvent {
                    forwarder_id: event.forwarder_id.clone(),
                    reader_ip: event.reader_ip.clone(),
                    stream_epoch: event.stream_epoch as u64,
                    seq: event.seq as u64,
                    reader_timestamp: event.reader_timestamp.clone().unwrap_or_default(),
                    raw_read_line: event.raw_read_line.clone(),
                    read_type: event.read_type.clone(),
                })
                .collect();

            let batch = WsMessage::ReceiverEventBatch(ReceiverEventBatch {
                session_id: session_id.to_owned(),
                events: read_events,
            });
            socket
                .send(Message::Text(serde_json::to_string(&batch)?.into()))
                .await?;

            let Some(last) = events.last() else {
                break;
            };
            if let Some(sub) = subscriptions
                .iter_mut()
                .find(|sub| sub.stream_id == target.stream_id)
                && cursor_gt(last.stream_epoch, last.seq, sub.last_epoch, sub.last_seq)
            {
                sub.last_epoch = last.stream_epoch;
                sub.last_seq = last.seq;
            }

            from_seq = last.seq.saturating_add(1);
            if events.len() < REPLAY_BATCH_LIMIT as usize {
                break;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        RaceBaseline, ResolvedStreamTarget, StreamSub, plan_race_refresh_subscriptions,
        race_refresh_needed,
    };
    use rt_protocol::ReadEvent;
    use std::collections::HashMap;
    use uuid::Uuid;

    fn target(stream_id: Uuid, current_stream_epoch: i64) -> ResolvedStreamTarget {
        ResolvedStreamTarget {
            stream_id,
            forwarder_id: "fwd".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            current_stream_epoch,
        }
    }

    fn baseline(entries: &[(Uuid, &[i64])]) -> RaceBaseline {
        let mut max_epochs: HashMap<Uuid, i64> = HashMap::new();
        for (stream_id, epochs) in entries {
            if let Some(max_epoch) = epochs.iter().copied().max() {
                max_epochs.insert(*stream_id, max_epoch);
            }
        }
        RaceBaseline { max_epochs }
    }

    fn sub(stream_id: Uuid) -> StreamSub {
        let (_, rx) = tokio::sync::broadcast::channel(1);
        StreamSub {
            stream_id,
            last_epoch: 0,
            last_seq: 0,
            rx,
        }
    }

    #[test]
    fn race_refresh_needed_when_same_stream_has_new_epoch() {
        let stream_id = Uuid::new_v4();
        let targets = vec![target(stream_id, 8)];
        let subscriptions = vec![sub(stream_id)];
        let baseline = baseline(&[(stream_id, &[7])]);

        assert!(
            race_refresh_needed(&targets, &subscriptions, &baseline),
            "same stream id with a new epoch must refresh"
        );
    }

    #[test]
    fn race_refresh_needed_when_stream_readded_with_known_epoch() {
        let stream_id = Uuid::new_v4();
        let targets = vec![target(stream_id, 7)];
        let subscriptions: Vec<StreamSub> = Vec::new();
        let baseline = baseline(&[(stream_id, &[7])]);

        assert!(
            race_refresh_needed(&targets, &subscriptions, &baseline),
            "stream re-add must refresh even when epoch was already known"
        );
    }

    #[test]
    fn race_refresh_plan_keeps_existing_subscription_receiver_buffer() {
        let stream_id = Uuid::new_v4();
        let (tx, rx) = tokio::sync::broadcast::channel(8);
        tx.send(ReadEvent {
            forwarder_id: "fwd".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            stream_epoch: 2,
            seq: 3,
            reader_timestamp: "2026-02-25T12:00:00.000Z".to_owned(),
            raw_read_line: "QUEUED_BEFORE_REFRESH".to_owned(),
            read_type: "RAW".to_owned(),
        })
        .unwrap();

        let subscriptions = vec![StreamSub {
            stream_id,
            last_epoch: 1,
            last_seq: 1,
            rx,
        }];
        let mut baseline = baseline(&[(stream_id, &[1])]);
        let targets = vec![target(stream_id, 2)];

        let (mut next_subscriptions, new_targets) =
            plan_race_refresh_subscriptions(targets, subscriptions, &mut baseline);

        assert!(
            new_targets.is_empty(),
            "existing stream should not be rebuilt"
        );
        let queued = next_subscriptions[0]
            .rx
            .try_recv()
            .expect("queued event should still be readable");
        assert_eq!(queued.raw_read_line, "QUEUED_BEFORE_REFRESH");
    }

    #[test]
    fn race_baseline_tracking_is_bounded_per_stream() {
        let stream_id = Uuid::new_v4();
        let mut baseline = RaceBaseline {
            max_epochs: HashMap::new(),
        };

        for epoch in 1..=512 {
            baseline.record(stream_id, epoch);
        }

        assert_eq!(
            baseline.max_epochs.get(&stream_id).copied(),
            Some(512),
            "baseline should keep bounded state per stream across many epoch updates"
        );
    }
}

async fn handle_receiver_ack(
    state: &AppState,
    device_id: &str,
    ack: ReceiverAck,
    persist_cursors: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if !persist_cursors {
        return Ok(());
    }

    for entry in &ack.entries {
        let row = sqlx::query!(
            "SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2",
            entry.forwarder_id,
            entry.reader_ip
        )
        .fetch_optional(&state.pool)
        .await?;

        if let Some(r) = row {
            upsert_cursor(
                &state.pool,
                device_id,
                r.stream_id,
                entry.stream_epoch as i64,
                entry.last_seq as i64,
            )
            .await?;
        }
    }

    Ok(())
}
