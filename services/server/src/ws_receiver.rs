use crate::{
    auth::{extract_bearer, validate_token},
    repo::{
        events::fetch_events_after_cursor,
        receiver_cursors::{fetch_cursor, upsert_cursor},
    },
    state::AppState,
};
use axum::extract::{
    ws::{Message, WebSocket, WebSocketUpgrade},
    State,
};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use rt_protocol::{error_codes, Heartbeat, ReadEvent, ReceiverAck, ReceiverEventBatch, WsMessage};
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const SESSION_TIMEOUT: Duration = Duration::from_secs(90);

pub async fn ws_receiver_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| extract_bearer(v))
        .map(|s| s.to_owned());
    ws.on_upgrade(move |socket| handle_receiver_socket(socket, state, token))
}

async fn send_ws_error(socket: &mut WebSocket, code: &str, message: &str, retryable: bool) {
    let msg = WsMessage::Error(rt_protocol::ErrorMessage {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable,
    });
    if let Ok(json) = serde_json::to_string(&msg) {
        let _ = socket.send(Message::Text(json)).await;
    }
}

/// Per-stream subscription state.
struct StreamSub {
    stream_id: Uuid,
    last_epoch: i64,
    last_seq: i64,
    rx: tokio::sync::broadcast::Receiver<ReadEvent>,
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
    let hello = match tokio::time::timeout(SESSION_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<WsMessage>(&text) {
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
        _ => {
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
    info!(device_id = %device_id, session_id = %session_id, "receiver connected");
    state.logger.log(format!(
        "receiver {device_id} connected (session {session_id})"
    ));

    // Send heartbeat with session_id
    let hb_msg = WsMessage::Heartbeat(Heartbeat {
        session_id: session_id.clone(),
        device_id: device_id.clone(),
    });
    if let Ok(json) = serde_json::to_string(&hb_msg) {
        if socket.send(Message::Text(json)).await.is_err() {
            return;
        }
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
                        sub.last_epoch = event.stream_epoch as i64;
                        sub.last_seq = event.seq as i64;
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
            if let Ok(json) = serde_json::to_string(&batch) {
                if socket.send(Message::Text(json)).await.is_err() {
                    break;
                }
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
                                for stream_ref in &sub_msg.streams {
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
                        info!(device_id = %device_id, "receiver disconnected");
                        state.logger.log(format!("receiver {device_id} disconnected"));
                        break;
                    }
                    Err(_) => {} // short timeout - continue polling broadcasts
                    Ok(Some(Err(e))) => { warn!(device_id = %device_id, error = %e, "WS error"); break; }
                    Ok(Some(Ok(_))) => {}
                }
            }
            _ = heartbeat_interval.tick() => {
                let hb = WsMessage::Heartbeat(Heartbeat { session_id: session_id.clone(), device_id: device_id.clone() });
                if let Ok(json) = serde_json::to_string(&hb) {
                    if socket.send(Message::Text(json)).await.is_err() { break; }
                }
            }
        }
    }

    info!(device_id = %device_id, "receiver session ended");
    state
        .logger
        .log(format!("receiver {device_id} session ended"));
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
    let events =
        fetch_events_after_cursor(&state.pool, sub.stream_id, sub.last_epoch, sub.last_seq).await?;
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
    socket.send(Message::Text(json)).await?;
    Ok(())
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
