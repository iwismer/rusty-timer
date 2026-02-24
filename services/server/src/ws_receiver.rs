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
    EpochScope, ReadEvent, ReceiverAck, ReceiverEventBatch, ReceiverSelection,
    ReceiverSelectionApplied, ReplayPolicy, ReplayTarget, StreamRef, WsMessage, error_codes,
};
use sqlx::Row;
use sqlx::types::Uuid as SqlUuid;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const SESSION_TIMEOUT: Duration = Duration::from_secs(90);
const REPLAY_BATCH_LIMIT: i64 = 500;
const RACE_SELECTION_REFRESH_INTERVAL: Duration = Duration::from_millis(500);

pub async fn ws_receiver_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = extract_token_from_headers(&headers);
    ws.on_upgrade(move |socket| handle_receiver_socket(socket, state, token))
}

pub async fn ws_receiver_v11_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = extract_token_from_headers(&headers);
    ws.on_upgrade(move |socket| handle_receiver_socket_v11(socket, state, token))
}

/// Per-stream subscription state.
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
    current_epoch_only: bool,
}

#[derive(Clone)]
struct PendingSelection {
    selection: ReceiverSelection,
    replay_policy: ReplayPolicy,
    replay_targets: Option<Vec<ReplayTarget>>,
}

struct TargetedReplaySelection {
    stream_id: Uuid,
    stream_epoch: i64,
    from_seq: i64,
    through_epoch: i64,
    through_seq: i64,
}

enum ApplySelectionOutcome {
    Applied,
    Replaced(PendingSelection),
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

    // Wait for ReceiverHello
    let hello = match recv_text_with_timeout(&mut socket, SESSION_TIMEOUT).await {
        Ok(text) => match serde_json::from_str::<WsMessage>(&text) {
            Ok(WsMessage::ReceiverHello(hello)) => hello,
            Ok(_) => {
                send_ws_error(
                    &mut socket,
                    error_codes::PROTOCOL_ERROR,
                    "expected receiver_hello",
                    false,
                )
                .await;
                return;
            }
            Err(e) => {
                send_ws_error(
                    &mut socket,
                    error_codes::PROTOCOL_ERROR,
                    &format!("invalid JSON: {}", e),
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
                "timeout waiting for receiver_hello",
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
    let mut legacy_selection_streams = normalize_manual_streams(
        hello
            .resume
            .iter()
            .map(|cursor| StreamRef {
                forwarder_id: cursor.forwarder_id.clone(),
                reader_ip: cursor.reader_ip.clone(),
            })
            .collect(),
    );
    state
        .register_receiver_session(
            &session_id,
            &device_id,
            ReceiverSessionProtocol::V1,
            ReceiverSelectionSnapshot::LegacyV1 {
                streams: legacy_selection_streams.clone(),
            },
        )
        .await;

    state.logger.log(format!(
        "receiver {device_id} connected (session {session_id})"
    ));

    let session_id_for_cleanup = session_id.clone();
    async {
        // Send heartbeat with session_id
        if !send_heartbeat(&mut socket, &session_id, &device_id).await {
            return;
        }

        let mut subscriptions: Vec<StreamSub> = Vec::new();

        // Process resume cursors from hello
        for cursor in &hello.resume {
            if let Some(sub) = subscribe_to_stream(
                &state,
                &cursor.forwarder_id,
                &cursor.reader_ip,
                cursor.stream_epoch as i64,
                cursor.last_seq as i64,
            )
            .await
            {
                subscriptions.push(sub);
            }
        }

        // Replay backlog for each subscribed stream
        for sub in &mut subscriptions {
            if let Err(e) = replay_backlog(&mut socket, &state, &session_id, sub).await {
                error!(device_id = %device_id, error = %e, "error during replay");
                return;
            }
        }

        let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat_interval.tick().await;

        loop {
            // Check all broadcasts for ready events (non-blocking)
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
                            warn!(device_id = %device_id, stream_id = %sub.stream_id, lagged = n, "receiver lagged, replaying from DB");
                            if let Err(e) = replay_backlog(&mut socket, &state, &session_id, sub).await
                            {
                                error!(error = %e, "replay failed");
                                return;
                            }
                            break;
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                            warn!(device_id = %device_id, stream_id = %sub.stream_id, "broadcast channel closed");
                            break;
                        }
                    }
                }
            }

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
                continue;
            }

            // Wait on socket or heartbeat with short timeout to allow broadcast polling
            let wait_duration = Duration::from_millis(10);
            tokio::select! {
                msg = tokio::time::timeout(wait_duration, socket.recv()) => {
                    match msg {
                        Ok(Some(Ok(Message::Text(text)))) => {
                            match serde_json::from_str::<WsMessage>(&text) {
                                Ok(WsMessage::ReceiverAck(ack)) => {
                                    if let Err(e) = handle_receiver_ack(&state, &device_id, ack).await {
                                        error!(device_id = %device_id, error = %e, "error handling receiver ack");
                                    }
                                }
                                Ok(WsMessage::ReceiverSubscribe(sub_msg)) => {
                                    let mut snapshot_updated = false;
                                    for stream_ref in &sub_msg.streams {
                                        if !legacy_selection_streams.iter().any(|stream| {
                                            stream.forwarder_id == stream_ref.forwarder_id
                                                && stream.reader_ip == stream_ref.reader_ip
                                        }) {
                                            legacy_selection_streams.push(StreamRef {
                                                forwarder_id: stream_ref.forwarder_id.clone(),
                                                reader_ip: stream_ref.reader_ip.clone(),
                                            });
                                            snapshot_updated = true;
                                        }
                                        let (last_epoch, last_seq) = get_cursor_for_stream(
                                            &state, &device_id,
                                            &stream_ref.forwarder_id, &stream_ref.reader_ip,
                                        ).await;
                                        if let Some(new_sub) = subscribe_to_stream(
                                            &state,
                                            &stream_ref.forwarder_id, &stream_ref.reader_ip,
                                            last_epoch, last_seq,
                                        ).await {
                                            let already = subscriptions.iter().any(|s| s.stream_id == new_sub.stream_id);
                                            if !already {
                                                subscriptions.push(new_sub);
                                                let last = subscriptions.last_mut().unwrap();
                                                if let Err(e) = replay_backlog(&mut socket, &state, &session_id, last).await {
                                                    error!(error = %e, "replay failed");
                                                    return;
                                                }
                                            }
                                        }
                                    }
                                    if snapshot_updated {
                                        let _ = state
                                            .update_receiver_session_selection(
                                                &session_id,
                                                ReceiverSelectionSnapshot::LegacyV1 {
                                                    streams: normalize_manual_streams(
                                                        legacy_selection_streams.clone(),
                                                    ),
                                                },
                                            )
                                            .await;
                                    }
                                }
                                Ok(WsMessage::Heartbeat(_)) => {}
                                Ok(_) => { warn!(device_id = %device_id, "unexpected message kind"); }
                                Err(e) => {
                                    send_ws_error(&mut socket, error_codes::PROTOCOL_ERROR, &format!("invalid JSON: {}", e), false).await;
                                    break;
                                }
                            }
                        }
                        Ok(Some(Ok(Message::Ping(data)))) => { let _ = socket.send(Message::Pong(data)).await; }
                        Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                            state.logger.log(format!("receiver {device_id} disconnected"));
                            break;
                        }
                        Err(_) => {} // short timeout - continue polling broadcasts
                        Ok(Some(Err(e))) => { warn!(device_id = %device_id, error = %e, "WS error"); break; }
                        Ok(Some(Ok(_))) => {}
                    }
                }
                _ = heartbeat_interval.tick() => {
                    if !send_heartbeat(&mut socket, &session_id, &device_id).await { break; }
                }
            }
        }

        info!(device_id = %device_id, "receiver session ended");
        state
            .logger
            .log(format!("receiver {device_id} session ended"));
    }
    .await;

    state
        .unregister_receiver_session(&session_id_for_cleanup)
        .await;
}

async fn handle_receiver_socket_v11(mut socket: WebSocket, state: AppState, token: Option<String>) {
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

    // Wait for ReceiverHelloV11.
    let hello = match recv_text_with_timeout(&mut socket, SESSION_TIMEOUT).await {
        Ok(text) => match serde_json::from_str::<WsMessage>(&text) {
            Ok(WsMessage::ReceiverHelloV11(hello)) => hello,
            Ok(_) => {
                send_ws_error(
                    &mut socket,
                    error_codes::PROTOCOL_ERROR,
                    "expected receiver_hello_v11",
                    false,
                )
                .await;
                return;
            }
            Err(e) => {
                send_ws_error(
                    &mut socket,
                    error_codes::PROTOCOL_ERROR,
                    &format!("invalid JSON: {}", e),
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
                "timeout waiting for receiver_hello_v11",
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
    state
        .register_receiver_session(
            &session_id,
            &device_id,
            ReceiverSessionProtocol::V11,
            selection_to_snapshot(&hello.selection),
        )
        .await;

    state.logger.log(format!(
        "receiver {device_id} connected (session {session_id}) [v1.1]"
    ));

    let session_id_for_cleanup = session_id.clone();
    async {
        if !send_heartbeat(&mut socket, &session_id, &device_id).await {
            return;
        }

        let mut subscriptions: Vec<StreamSub> = Vec::new();
        let mut active_selection = match apply_receiver_selection_until_stable(
            &mut socket,
            &state,
            &device_id,
            &session_id,
            PendingSelection {
                selection: hello.selection,
                replay_policy: hello.replay_policy,
                replay_targets: hello.replay_targets,
            },
            &mut subscriptions,
        )
        .await
        {
            Ok(applied) => applied,
            Err(e) => {
                error!(device_id = %device_id, error = %e, "error applying initial receiver selection");
                return;
            }
        };

        let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat_interval.tick().await;
        let mut race_refresh_interval = tokio::time::interval(RACE_SELECTION_REFRESH_INTERVAL);
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
                            warn!(device_id = %device_id, stream_id = %sub.stream_id, lagged = n, "receiver lagged, replaying from DB");
                            if let Err(e) = replay_backlog(&mut socket, &state, &session_id, sub).await
                            {
                                error!(error = %e, "replay failed");
                                return;
                            }
                            break;
                        }
                        Err(tokio::sync::broadcast::error::TryRecvError::Closed) => {
                            warn!(device_id = %device_id, stream_id = %sub.stream_id, "broadcast channel closed");
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
                                    if let Err(e) = handle_receiver_ack(&state, &device_id, ack).await {
                                        error!(device_id = %device_id, error = %e, "error handling receiver ack");
                                    }
                                }
                                Ok(WsMessage::ReceiverSetSelection(set_selection)) => {
                                    match apply_receiver_selection_until_stable(
                                        &mut socket,
                                        &state,
                                        &device_id,
                                        &session_id,
                                        PendingSelection {
                                            selection: set_selection.selection,
                                            replay_policy: set_selection.replay_policy,
                                            replay_targets: set_selection.replay_targets,
                                        },
                                        &mut subscriptions,
                                    )
                                    .await
                                    {
                                        Ok(applied) => {
                                            active_selection = applied;
                                        }
                                        Err(e) => {
                                            error!(device_id = %device_id, error = %e, "error applying receiver set_selection");
                                            break;
                                        }
                                    }
                                }
                                Ok(WsMessage::Heartbeat(_)) => {}
                                Ok(_) => { warn!(device_id = %device_id, "unexpected message kind"); }
                                Err(e) => {
                                    send_ws_error(&mut socket, error_codes::PROTOCOL_ERROR, &format!("invalid JSON: {}", e), false).await;
                                    break;
                                }
                            }
                        }
                        Ok(Some(Ok(Message::Ping(data)))) => { let _ = socket.send(Message::Pong(data)).await; }
                        Ok(Some(Ok(Message::Close(_)))) | Ok(None) => {
                            state.logger.log(format!("receiver {device_id} disconnected"));
                            break;
                        }
                        Err(_) => {}
                        Ok(Some(Err(e))) => { warn!(device_id = %device_id, error = %e, "WS error"); break; }
                        Ok(Some(Ok(_))) => {}
                    }
                }
                _ = heartbeat_interval.tick() => {
                    if !send_heartbeat(&mut socket, &session_id, &device_id).await { break; }
                }
                _ = race_refresh_interval.tick() => {
                    match selection_refresh_needed(&state, &active_selection, &subscriptions).await {
                        Ok(true) => {
                            info!(device_id = %device_id, "race/current selection targets changed; refreshing");
                            match apply_receiver_selection_until_stable(
                                &mut socket,
                                &state,
                                &device_id,
                                &session_id,
                                active_selection.clone(),
                                &mut subscriptions,
                            )
                            .await
                            {
                                Ok(applied) => {
                                    active_selection = applied;
                                }
                                Err(e) => {
                                    error!(device_id = %device_id, error = %e, "error refreshing race/current receiver selection");
                                    break;
                                }
                            }
                        }
                        Ok(false) => {}
                        Err(e) => {
                            error!(device_id = %device_id, error = %e, "error checking race/current selection refresh");
                            break;
                        }
                    }
                }
            }
        }
        info!(device_id = %device_id, "receiver v1.1 session ended");
        state
            .logger
            .log(format!("receiver {device_id} v1.1 session ended"));
    }
    .await;

    state
        .unregister_receiver_session(&session_id_for_cleanup)
        .await;
}

async fn apply_receiver_selection_until_stable(
    socket: &mut WebSocket,
    state: &AppState,
    device_id: &str,
    session_id: &str,
    pending: PendingSelection,
    subscriptions: &mut Vec<StreamSub>,
) -> Result<PendingSelection, Box<dyn std::error::Error + Send + Sync>> {
    let mut next_pending = Some(pending);
    while let Some(current) = next_pending.take() {
        match apply_receiver_selection(
            socket,
            state,
            device_id,
            session_id,
            current.clone(),
            subscriptions,
        )
        .await?
        {
            ApplySelectionOutcome::Applied => return Ok(current),
            ApplySelectionOutcome::Replaced(replacement) => {
                // Drop in-memory cursors before retrying so replacement replay
                // always rehydrates from persisted cursor rows.
                subscriptions.clear();
                info!(
                    device_id = %device_id,
                    "selection changed during replay; restarting with latest selection"
                );
                next_pending = Some(replacement);
            }
        }
    }
    Err("selection apply loop ended without applying a selection".into())
}

async fn apply_receiver_selection(
    socket: &mut WebSocket,
    state: &AppState,
    device_id: &str,
    session_id: &str,
    pending: PendingSelection,
    subscriptions: &mut Vec<StreamSub>,
) -> Result<ApplySelectionOutcome, Box<dyn std::error::Error + Send + Sync>> {
    let PendingSelection {
        selection,
        replay_policy,
        replay_targets,
    } = pending;
    let selection_snapshot = selection_to_snapshot(&selection);
    let mut warnings: Vec<String> = Vec::new();
    let resolved = resolve_selection_targets(state, selection).await?;
    if matches!(
        selection_snapshot,
        ReceiverSelectionSnapshot::Race {
            epoch_scope: EpochScope::Current,
            ..
        }
    ) && resolved.is_empty()
    {
        warnings.push(
            "current-scope mismatch: no streams currently resolve for selected race".to_owned(),
        );
    }
    let targeted_replay_selections = if replay_policy == ReplayPolicy::Targeted {
        resolve_targeted_replay_targets(&resolved, replay_targets, &mut warnings)
    } else {
        Vec::new()
    };
    let normalized_streams = resolved
        .iter()
        .map(|target| StreamRef {
            forwarder_id: target.forwarder_id.clone(),
            reader_ip: target.reader_ip.clone(),
        })
        .collect::<Vec<_>>();

    let mut next_subscriptions = Vec::with_capacity(resolved.len());
    for target in resolved {
        let (last_epoch, last_seq) =
            compute_selection_start_cursor(state, device_id, &target, replay_policy).await?;
        let tx = state.get_or_create_broadcast(target.stream_id).await;
        let rx = tx.subscribe();
        next_subscriptions.push(StreamSub {
            stream_id: target.stream_id,
            last_epoch,
            last_seq,
            rx,
        });
    }

    if replay_policy == ReplayPolicy::Resume {
        for sub in &mut next_subscriptions {
            if let Some(replacement) =
                replay_backlog_interruptible(socket, state, device_id, session_id, sub).await?
            {
                return Ok(ApplySelectionOutcome::Replaced(replacement));
            }
        }
    } else if replay_policy == ReplayPolicy::Targeted {
        let targeted_replay =
            snapshot_targeted_replay_bounds(state, targeted_replay_selections).await?;
        if let Some(replacement) = replay_targeted_backlog(
            socket,
            state,
            device_id,
            session_id,
            &targeted_replay,
            &mut next_subscriptions,
        )
        .await?
        {
            return Ok(ApplySelectionOutcome::Replaced(replacement));
        }
    }

    *subscriptions = next_subscriptions;

    let applied = WsMessage::ReceiverSelectionApplied(ReceiverSelectionApplied {
        selection: ReceiverSelection::Manual {
            streams: normalize_manual_streams(normalized_streams),
        },
        replay_policy,
        resolved_target_count: subscriptions.len(),
        warnings,
    });
    let payload = serde_json::to_string(&applied)?;
    if socket.send(Message::Text(payload.into())).await.is_err() {
        return Err("failed to send receiver_selection_applied".into());
    }
    state.logger.log(format!(
        "receiver {device_id} selection applied ({} streams)",
        subscriptions.len()
    ));
    let _ = state
        .update_receiver_session_selection(session_id, selection_snapshot)
        .await;
    Ok(ApplySelectionOutcome::Applied)
}

async fn selection_refresh_needed(
    state: &AppState,
    selection: &PendingSelection,
    subscriptions: &[StreamSub],
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let ReceiverSelection::Race {
        epoch_scope: EpochScope::Current,
        ..
    } = &selection.selection
    else {
        return Ok(false);
    };

    let resolved = resolve_selection_targets(state, selection.selection.clone()).await?;
    let mut resolved_stream_ids = resolved
        .into_iter()
        .map(|target| target.stream_id)
        .collect::<Vec<_>>();
    let mut subscribed_stream_ids = subscriptions
        .iter()
        .map(|sub| sub.stream_id)
        .collect::<Vec<_>>();
    resolved_stream_ids.sort_unstable();
    subscribed_stream_ids.sort_unstable();
    Ok(resolved_stream_ids != subscribed_stream_ids)
}

fn selection_to_snapshot(selection: &ReceiverSelection) -> ReceiverSelectionSnapshot {
    match selection {
        ReceiverSelection::Manual { streams } => ReceiverSelectionSnapshot::Manual {
            streams: normalize_manual_streams(streams.clone()),
        },
        ReceiverSelection::Race {
            race_id,
            epoch_scope,
        } => ReceiverSelectionSnapshot::Race {
            race_id: race_id.clone(),
            epoch_scope: *epoch_scope,
        },
    }
}

async fn snapshot_targeted_replay_bounds(
    state: &AppState,
    targeted: Vec<TargetedReplaySelection>,
) -> Result<Vec<TargetedReplaySelection>, Box<dyn std::error::Error + Send + Sync>> {
    let mut bounded = Vec::with_capacity(targeted.len());
    for target in targeted {
        let Some((through_epoch, through_seq)) =
            fetch_max_event_cursor(&state.pool, target.stream_id).await?
        else {
            continue;
        };
        bounded.push(TargetedReplaySelection {
            stream_id: target.stream_id,
            stream_epoch: target.stream_epoch,
            from_seq: target.from_seq,
            through_epoch,
            through_seq,
        });
    }
    Ok(bounded)
}

fn resolve_targeted_replay_targets(
    resolved: &[ResolvedStreamTarget],
    replay_targets: Option<Vec<ReplayTarget>>,
    warnings: &mut Vec<String>,
) -> Vec<TargetedReplaySelection> {
    let by_stream_ref: HashMap<(&str, &str), &ResolvedStreamTarget> = resolved
        .iter()
        .map(|target| {
            (
                (target.forwarder_id.as_str(), target.reader_ip.as_str()),
                target,
            )
        })
        .collect();

    let Some(replay_targets) = replay_targets else {
        return Vec::new();
    };

    let mut dedup = HashSet::new();
    let mut targeted = Vec::new();
    for target in replay_targets {
        let Some(resolved_target) = by_stream_ref
            .get(&(target.forwarder_id.as_str(), target.reader_ip.as_str()))
            .copied()
        else {
            warnings.push(format!(
                "ignored replay_target for unselected stream {}:{}",
                target.forwarder_id, target.reader_ip
            ));
            continue;
        };
        let from_seq = target.from_seq.max(1);
        let dedup_key = (resolved_target.stream_id, target.stream_epoch, from_seq);
        if dedup.insert(dedup_key) {
            targeted.push(TargetedReplaySelection {
                stream_id: resolved_target.stream_id,
                stream_epoch: target.stream_epoch,
                from_seq,
                through_epoch: 0,
                through_seq: 0,
            });
        }
    }
    targeted
}

async fn resolve_selection_targets(
    state: &AppState,
    selection: ReceiverSelection,
) -> Result<Vec<ResolvedStreamTarget>, Box<dyn std::error::Error + Send + Sync>> {
    match selection {
        ReceiverSelection::Manual { streams } => {
            let mut dedup = HashSet::new();
            let mut targets = Vec::new();
            for stream in normalize_manual_streams(streams) {
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
                        forwarder_id: stream.forwarder_id,
                        reader_ip: stream.reader_ip,
                        current_stream_epoch: row.get("stream_epoch"),
                        current_epoch_only: false,
                    });
                }
            }
            Ok(targets)
        }
        ReceiverSelection::Race {
            race_id,
            epoch_scope,
        } => {
            let Ok(race_uuid) = SqlUuid::parse_str(&race_id) else {
                return Ok(Vec::new());
            };
            let rows = list_race_selection_streams(
                &state.pool,
                race_uuid,
                epoch_scope == EpochScope::Current,
            )
            .await?;
            let mut targets = Vec::with_capacity(rows.len());
            for row in rows {
                targets.push(ResolvedStreamTarget {
                    stream_id: row.stream_id,
                    forwarder_id: row.forwarder_id,
                    reader_ip: row.reader_ip,
                    current_stream_epoch: row.stream_epoch,
                    current_epoch_only: epoch_scope == EpochScope::Current,
                });
            }
            Ok(targets)
        }
    }
}

fn normalize_manual_streams(streams: Vec<StreamRef>) -> Vec<StreamRef> {
    let mut dedup = HashSet::new();
    let mut normalized = Vec::new();
    for stream in streams {
        if dedup.insert((stream.forwarder_id.clone(), stream.reader_ip.clone())) {
            normalized.push(stream);
        }
    }
    normalized
}

async fn compute_selection_start_cursor(
    state: &AppState,
    device_id: &str,
    target: &ResolvedStreamTarget,
    replay_policy: ReplayPolicy,
) -> Result<(i64, i64), Box<dyn std::error::Error + Send + Sync>> {
    match replay_policy {
        ReplayPolicy::Resume => {
            let cursor = fetch_cursor(&state.pool, device_id, target.stream_id).await?;
            if target.current_epoch_only {
                Ok(match cursor {
                    Some((epoch, seq)) if epoch == target.current_stream_epoch => (epoch, seq),
                    Some(_) => (target.current_stream_epoch, 0),
                    None => match fetch_max_event_cursor(&state.pool, target.stream_id).await? {
                        // Current-only resume must not replay old epochs.
                        // When the stream head is stale (< current_stream_epoch), we still start
                        // from the stale head so first live stale-epoch events are not dropped.
                        Some((tail_epoch, tail_seq))
                            if tail_epoch < target.current_stream_epoch =>
                        {
                            (tail_epoch, tail_seq)
                        }
                        // Otherwise begin at current epoch start so first-time receivers replay
                        // already persisted current-epoch rows.
                        _ => (target.current_stream_epoch, 0),
                    },
                })
            } else {
                Ok(cursor.unwrap_or((1, 0)))
            }
        }
        ReplayPolicy::LiveOnly | ReplayPolicy::Targeted => {
            let tail = fetch_max_event_cursor(&state.pool, target.stream_id).await?;
            // When no events exist yet, use (1, 0) rather than
            // (current_stream_epoch, 0).  If the stream_epoch was advanced
            // ahead of actual event ingestion the higher fallback would
            // silently filter every stale-epoch event via cursor_gt.
            Ok(tail.unwrap_or((1, 0)))
        }
    }
}

async fn subscribe_to_stream(
    state: &AppState,
    forwarder_id: &str,
    reader_ip: &str,
    last_epoch: i64,
    last_seq: i64,
) -> Option<StreamSub> {
    let row = sqlx::query!(
        "SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2",
        forwarder_id,
        reader_ip
    )
    .fetch_optional(&state.pool)
    .await
    .ok()??;

    let stream_id = row.stream_id;
    let tx = state.get_or_create_broadcast(stream_id).await;
    let rx = tx.subscribe();

    Some(StreamSub {
        stream_id,
        last_epoch,
        last_seq,
        rx,
    })
}

async fn get_cursor_for_stream(
    state: &AppState,
    device_id: &str,
    forwarder_id: &str,
    reader_ip: &str,
) -> (i64, i64) {
    let row = sqlx::query!(
        "SELECT stream_id FROM streams WHERE forwarder_id = $1 AND reader_ip = $2",
        forwarder_id,
        reader_ip
    )
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();

    let Some(stream_row) = row else {
        return (1, 0);
    };
    let stream_id = stream_row.stream_id;

    fetch_cursor(&state.pool, device_id, stream_id)
        .await
        .ok()
        .flatten()
        .unwrap_or((1, 0))
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
            .map(|e| ReadEvent {
                forwarder_id: e.forwarder_id.clone(),
                reader_ip: e.reader_ip.clone(),
                stream_epoch: e.stream_epoch as u64,
                seq: e.seq as u64,
                reader_timestamp: e.reader_timestamp.clone().unwrap_or_default(),
                raw_read_line: e.raw_read_line.clone(),
                read_type: e.read_type.clone(),
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
        let json = serde_json::to_string(&batch)?;
        socket.send(Message::Text(json.into())).await?;

        if !cursor_gt(through_epoch, through_seq, sub.last_epoch, sub.last_seq) {
            return Ok(());
        }

        if events.len() < REPLAY_BATCH_LIMIT as usize {
            return Ok(());
        }
    }
}

async fn replay_backlog_interruptible(
    socket: &mut WebSocket,
    state: &AppState,
    device_id: &str,
    session_id: &str,
    sub: &mut StreamSub,
) -> Result<Option<PendingSelection>, Box<dyn std::error::Error + Send + Sync>> {
    let Some((through_epoch, through_seq)) =
        fetch_max_event_cursor(&state.pool, sub.stream_id).await?
    else {
        return Ok(None);
    };
    if !cursor_gt(through_epoch, through_seq, sub.last_epoch, sub.last_seq) {
        return Ok(None);
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
            return Ok(None);
        }

        let read_events: Vec<ReadEvent> = events
            .iter()
            .map(|e| ReadEvent {
                forwarder_id: e.forwarder_id.clone(),
                reader_ip: e.reader_ip.clone(),
                stream_epoch: e.stream_epoch as u64,
                seq: e.seq as u64,
                reader_timestamp: e.reader_timestamp.clone().unwrap_or_default(),
                raw_read_line: e.raw_read_line.clone(),
                read_type: e.read_type.clone(),
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
        let json = serde_json::to_string(&batch)?;
        socket.send(Message::Text(json.into())).await?;

        if let Some(replacement) = poll_replay_control_messages(socket, state, device_id).await? {
            return Ok(Some(replacement));
        }

        if !cursor_gt(through_epoch, through_seq, sub.last_epoch, sub.last_seq) {
            return Ok(None);
        }

        if events.len() < REPLAY_BATCH_LIMIT as usize {
            return Ok(None);
        }
    }
}

async fn replay_targeted_backlog(
    socket: &mut WebSocket,
    state: &AppState,
    device_id: &str,
    session_id: &str,
    targets: &[TargetedReplaySelection],
    subscriptions: &mut [StreamSub],
) -> Result<Option<PendingSelection>, Box<dyn std::error::Error + Send + Sync>> {
    for target in targets {
        if target.through_epoch < target.stream_epoch
            || (target.through_epoch == target.stream_epoch && target.through_seq < target.from_seq)
        {
            continue;
        }
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
            let payload = serde_json::to_string(&batch)?;
            socket.send(Message::Text(payload.into())).await?;
            if let Some(replacement) =
                poll_replay_control_messages(socket, state, device_id).await?
            {
                return Ok(Some(replacement));
            }

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
    Ok(None)
}

async fn poll_replay_control_messages(
    socket: &mut WebSocket,
    state: &AppState,
    device_id: &str,
) -> Result<Option<PendingSelection>, Box<dyn std::error::Error + Send + Sync>> {
    let mut replacement: Option<PendingSelection> = None;
    loop {
        let maybe_msg = tokio::time::timeout(Duration::from_millis(0), socket.recv()).await;
        let msg = match maybe_msg {
            Ok(msg) => msg,
            Err(_) => return Ok(replacement),
        };
        match msg {
            Some(Ok(Message::Text(text))) => match serde_json::from_str::<WsMessage>(&text) {
                Ok(WsMessage::ReceiverSetSelection(set_selection)) => {
                    replacement = Some(PendingSelection {
                        selection: set_selection.selection,
                        replay_policy: set_selection.replay_policy,
                        replay_targets: set_selection.replay_targets,
                    });
                }
                Ok(WsMessage::ReceiverAck(ack)) => {
                    if let Err(e) = handle_receiver_ack(state, device_id, ack).await {
                        error!(device_id = %device_id, error = %e, "error handling receiver ack");
                    }
                }
                Ok(WsMessage::Heartbeat(_)) => {}
                Ok(other) => {
                    warn!(device_id = %device_id, message = ?other, "unexpected message kind during replay");
                }
                Err(e) => {
                    send_ws_error(
                        socket,
                        error_codes::PROTOCOL_ERROR,
                        &format!("invalid JSON: {}", e),
                        false,
                    )
                    .await;
                    return Err("invalid JSON during replay".into());
                }
            },
            Some(Ok(Message::Ping(data))) => {
                let _ = socket.send(Message::Pong(data)).await;
            }
            Some(Ok(Message::Close(_))) | None => {
                return Err("socket closed during replay".into());
            }
            Some(Err(e)) => return Err(format!("WS error during replay: {e}").into()),
            Some(Ok(_)) => {}
        }
    }
}

async fn handle_receiver_ack(
    state: &AppState,
    device_id: &str,
    ack: ReceiverAck,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
