use crate::{
    auth::{extract_bearer, validate_token},
    dashboard_events::DashboardEvent,
    repo::events::{
        count_unique_chips, fetch_stream_metrics, fetch_stream_snapshot, set_stream_online,
        upsert_event, upsert_stream, IngestResult,
    },
    state::AppState,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::HeaderMap,
    response::IntoResponse,
};
use rt_protocol::{error_codes, AckEntry, EpochResetCommand, ForwarderAck, Heartbeat, WsMessage};
use std::collections::HashMap;
use std::time::Duration;
use tracing::{error, info, warn};
use uuid::Uuid;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const SESSION_TIMEOUT: Duration = Duration::from_secs(90);

pub async fn ws_forwarder_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| extract_bearer(v))
        .map(|s| s.to_owned());
    ws.on_upgrade(move |socket| handle_forwarder_socket(socket, state, token))
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

async fn publish_stream_created(state: &AppState, stream_id: Uuid) {
    if let Ok(Some(stream)) = fetch_stream_snapshot(&state.pool, stream_id).await {
        let _ = state.dashboard_tx.send(DashboardEvent::StreamCreated {
            stream_id: stream.stream_id,
            forwarder_id: stream.forwarder_id,
            reader_ip: stream.reader_ip,
            display_alias: stream.display_alias,
            forwarder_display_name: stream.forwarder_display_name,
            online: stream.online,
            stream_epoch: stream.stream_epoch,
            created_at: stream.created_at.to_rfc3339(),
        });
    }
}

async fn handle_forwarder_socket(mut socket: WebSocket, state: AppState, token: Option<String>) {
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
    if claims.device_type != "forwarder" {
        send_ws_error(
            &mut socket,
            error_codes::INVALID_TOKEN,
            "token is not for a forwarder device",
            false,
        )
        .await;
        return;
    }
    let device_id = claims.device_id.clone();
    if !state.register_forwarder(&device_id).await {
        send_ws_error(
            &mut socket,
            error_codes::PROTOCOL_ERROR,
            "a session for this device is already active",
            false,
        )
        .await;
        return;
    }
    info!(device_id = %device_id, "forwarder connected");
    let session_id = Uuid::new_v4().to_string();

    let hello = match tokio::time::timeout(SESSION_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<WsMessage>(&text) {
            Ok(WsMessage::ForwarderHello(hello)) => hello,
            Ok(_) => {
                send_ws_error(
                    &mut socket,
                    error_codes::PROTOCOL_ERROR,
                    "expected forwarder_hello",
                    false,
                )
                .await;
                state.unregister_forwarder(&device_id).await;
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
                state.unregister_forwarder(&device_id).await;
                return;
            }
        },
        _ => {
            send_ws_error(
                &mut socket,
                error_codes::PROTOCOL_ERROR,
                "timeout waiting for forwarder_hello",
                false,
            )
            .await;
            state.unregister_forwarder(&device_id).await;
            return;
        }
    };

    if !hello.forwarder_id.is_empty() && hello.forwarder_id != device_id {
        send_ws_error(
            &mut socket,
            error_codes::IDENTITY_MISMATCH,
            "hello forwarder_id does not match token claims",
            false,
        )
        .await;
        state.unregister_forwarder(&device_id).await;
        return;
    }

    let mut current_display_name = hello.display_name.clone();
    let mut stream_map: HashMap<String, Uuid> = HashMap::new();
    for reader_ip in &hello.reader_ips {
        if let Ok(sid) = upsert_stream(
            &state.pool,
            &device_id,
            reader_ip,
            current_display_name.as_deref(),
        )
        .await
        {
            stream_map.insert(reader_ip.clone(), sid);
            let _ = set_stream_online(&state.pool, sid, true).await;
            state.get_or_create_broadcast(sid).await;
        }
    }

    // Notify dashboard of streams coming online
    for &sid in stream_map.values() {
        publish_stream_created(&state, sid).await;
    }

    let hb_msg = WsMessage::Heartbeat(Heartbeat {
        session_id: session_id.clone(),
        device_id: device_id.clone(),
    });
    if let Ok(json) = serde_json::to_string(&hb_msg) {
        if socket.send(Message::Text(json)).await.is_err() {
            state.unregister_forwarder(&device_id).await;
            return;
        }
    }

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<EpochResetCommand>(8);
    {
        let mut senders = state.forwarder_command_senders.write().await;
        senders.insert(device_id.clone(), cmd_tx);
    }

    let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat_interval.tick().await;

    loop {
        tokio::select! {
            msg = tokio::time::timeout(SESSION_TIMEOUT, socket.recv()) => {
                match msg {
                    Ok(Some(Ok(Message::Text(text)))) => {
                        match serde_json::from_str::<WsMessage>(&text) {
                            Ok(WsMessage::ForwarderEventBatch(batch)) => {
                                if let Err(e) = handle_event_batch(
                                    &mut socket,
                                    &state,
                                    &device_id,
                                    &session_id,
                                    &mut stream_map,
                                    current_display_name.as_deref(),
                                    batch,
                                )
                                .await
                                {
                                    error!(device_id = %device_id, error = %e, "error handling event batch"); break;
                                }
                            }
                            Ok(WsMessage::ForwarderHello(new_hello)) => {
                                current_display_name = new_hello.display_name.clone();
                                for reader_ip in &new_hello.reader_ips {
                                    if let Ok(sid) = upsert_stream(
                                        &state.pool,
                                        &device_id,
                                        reader_ip,
                                        current_display_name.as_deref(),
                                    )
                                    .await
                                    {
                                        if !stream_map.contains_key(reader_ip) {
                                            stream_map.insert(reader_ip.clone(), sid);
                                            let _ = set_stream_online(&state.pool, sid, true).await;
                                            state.get_or_create_broadcast(sid).await;
                                            publish_stream_created(&state, sid).await;
                                        }
                                    }
                                }
                                let hb = WsMessage::Heartbeat(Heartbeat { session_id: session_id.clone(), device_id: device_id.clone() });
                                if let Ok(json) = serde_json::to_string(&hb) { if socket.send(Message::Text(json)).await.is_err() { break; } }
                            }
                            Ok(WsMessage::Heartbeat(_)) => {}
                            Ok(_) => { warn!(device_id = %device_id, "unexpected message kind"); }
                            Err(e) => { send_ws_error(&mut socket, error_codes::PROTOCOL_ERROR, &format!("invalid JSON: {}", e), false).await; break; }
                        }
                    }
                    Ok(Some(Ok(Message::Ping(data)))) => { let _ = socket.send(Message::Pong(data)).await; }
                    Ok(Some(Ok(Message::Close(_)))) | Ok(None) => { info!(device_id = %device_id, "forwarder disconnected"); break; }
                    Err(_) => { warn!(device_id = %device_id, "session timeout"); break; }
                    Ok(Some(Err(e))) => { warn!(device_id = %device_id, error = %e, "WS error"); break; }
                    Ok(Some(Ok(_))) => {}
                }
            }
            _ = heartbeat_interval.tick() => {
                let hb = WsMessage::Heartbeat(Heartbeat { session_id: session_id.clone(), device_id: device_id.clone() });
                if let Ok(json) = serde_json::to_string(&hb) { if socket.send(Message::Text(json)).await.is_err() { break; } }
            }
            Some(cmd) = cmd_rx.recv() => {
                let msg = WsMessage::EpochResetCommand(cmd);
                if let Ok(json) = serde_json::to_string(&msg) { if socket.send(Message::Text(json)).await.is_err() { break; } }
            }
        }
    }

    for sid in stream_map.values() {
        let _ = set_stream_online(&state.pool, *sid, false).await;
        let _ = state.dashboard_tx.send(DashboardEvent::StreamUpdated {
            stream_id: *sid,
            online: Some(false),
            stream_epoch: None,
            display_alias: None,
        });
    }
    {
        let mut senders = state.forwarder_command_senders.write().await;
        senders.remove(&device_id);
    }
    state.unregister_forwarder(&device_id).await;
    info!(device_id = %device_id, "forwarder session ended");
}

async fn handle_event_batch(
    socket: &mut WebSocket,
    state: &AppState,
    device_id: &str,
    session_id: &str,
    stream_map: &mut HashMap<String, Uuid>,
    forwarder_display_name: Option<&str>,
    batch: rt_protocol::ForwarderEventBatch,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut high_water: HashMap<(String, u64), u64> = HashMap::new();
    let mut had_conflict = false;
    let mut epoch_transitions: HashMap<Uuid, i64> = HashMap::new();

    for event in &batch.events {
        let stream_id = if let Some(&sid) = stream_map.get(&event.reader_ip) {
            sid
        } else {
            let sid = upsert_stream(
                &state.pool,
                device_id,
                &event.reader_ip,
                forwarder_display_name,
            )
            .await?;
            stream_map.insert(event.reader_ip.clone(), sid);
            let _ = set_stream_online(&state.pool, sid, true).await;
            state.get_or_create_broadcast(sid).await;
            sid
        };

        let result = upsert_event(
            &state.pool,
            stream_id,
            event.stream_epoch as i64,
            event.seq as i64,
            &event.reader_timestamp,
            &event.raw_read_line,
            &event.read_type,
        )
        .await?;

        if let Some(new_epoch) = result.epoch_advanced_to {
            epoch_transitions.insert(stream_id, new_epoch);
        }

        match result.ingest_result {
            IngestResult::Inserted => {
                let tx = state.get_or_create_broadcast(stream_id).await;
                let _ = tx.send(event.clone());
                let entry = high_water
                    .entry((event.reader_ip.clone(), event.stream_epoch))
                    .or_insert(0);
                if event.seq > *entry {
                    *entry = event.seq;
                }
            }
            IngestResult::Retransmit => {
                let entry = high_water
                    .entry((event.reader_ip.clone(), event.stream_epoch))
                    .or_insert(0);
                if event.seq > *entry {
                    *entry = event.seq;
                }
            }
            IngestResult::IntegrityConflict => {
                had_conflict = true;
            }
        }
    }

    if had_conflict {
        let msg = WsMessage::Error(rt_protocol::ErrorMessage {
            code: error_codes::INTEGRITY_CONFLICT.to_owned(),
            message: "one or more events had mismatched payload for an existing key".to_owned(),
            retryable: false,
        });
        if let Ok(json) = serde_json::to_string(&msg) {
            socket.send(Message::Text(json)).await?;
        }
        return Ok(());
    }

    let entries: Vec<AckEntry> = high_water
        .into_iter()
        .map(|((reader_ip, stream_epoch), last_seq)| AckEntry {
            forwarder_id: device_id.to_owned(),
            reader_ip,
            stream_epoch,
            last_seq,
        })
        .collect();
    let ack = WsMessage::ForwarderAck(ForwarderAck {
        session_id: session_id.to_owned(),
        entries,
    });
    if let Ok(json) = serde_json::to_string(&ack) {
        socket.send(Message::Text(json)).await?;
    }

    for (stream_id, new_epoch) in epoch_transitions {
        let _ = state.dashboard_tx.send(DashboardEvent::StreamUpdated {
            stream_id,
            online: None,
            stream_epoch: Some(new_epoch),
            display_alias: None,
        });
    }

    // Notify dashboard of updated metrics
    let touched_streams: std::collections::HashSet<Uuid> = batch
        .events
        .iter()
        .filter_map(|e| stream_map.get(&e.reader_ip).copied())
        .collect();
    for sid in touched_streams {
        let m = match fetch_stream_metrics(&state.pool, sid).await {
            Ok(Some(metrics)) => metrics,
            Ok(None) => continue,
            Err(e) => {
                error!(
                    stream_id = %sid,
                    error = %e,
                    "failed to fetch stream metrics for dashboard update"
                );
                continue;
            }
        };

        let epoch = match sqlx::query_scalar::<_, i64>(
            "SELECT stream_epoch FROM streams WHERE stream_id = $1",
        )
        .bind(sid)
        .fetch_optional(&state.pool)
        .await
        {
            Ok(Some(epoch)) => epoch,
            Ok(None) => continue,
            Err(e) => {
                error!(
                    stream_id = %sid,
                    error = %e,
                    "failed to fetch stream epoch for dashboard update"
                );
                continue;
            }
        };

        let unique_chips = match count_unique_chips(&state.pool, sid, epoch).await {
            Ok(count) => count,
            Err(e) => {
                error!(
                    stream_id = %sid,
                    epoch,
                    error = %e,
                    "failed to count unique chips for dashboard update"
                );
                continue;
            }
        };

        let _ = state.dashboard_tx.send(DashboardEvent::MetricsUpdated {
            stream_id: sid,
            raw_count: m.raw_count,
            dedup_count: m.dedup_count,
            retransmit_count: m.retransmit_count,
            lag_ms: m.lag_ms,
            epoch_raw_count: m.epoch_raw_count,
            epoch_dedup_count: m.epoch_dedup_count,
            epoch_retransmit_count: m.epoch_retransmit_count,
            epoch_lag_ms: m.epoch_lag_ms,
            epoch_last_received_at: m.epoch_last_received_at.map(|ts| ts.to_rfc3339()),
            unique_chips,
        });
    }

    Ok(())
}
