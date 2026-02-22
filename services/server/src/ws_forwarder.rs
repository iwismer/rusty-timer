use crate::{
    auth::validate_token,
    dashboard_events::{DashboardEvent, OptionalStringPatch},
    repo::events::{
        count_unique_chips, fetch_stream_ids_by_forwarder, fetch_stream_metrics,
        fetch_stream_snapshot, set_stream_online, update_forwarder_display_name, upsert_event,
        upsert_stream, IngestResult,
    },
    state::{AppState, ForwarderCommand, ForwarderProxyReply},
    ws_common::{
        extract_token_from_headers, recv_text_with_timeout, send_heartbeat, send_ws_error,
    },
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::HeaderMap,
    response::IntoResponse,
};
use rt_protocol::{error_codes, AckEntry, ForwarderAck, WsMessage};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};
use uuid::Uuid;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const SESSION_TIMEOUT: Duration = Duration::from_secs(90);
const FORWARDER_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

fn is_path_safe_forwarder_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~'))
}

pub async fn ws_forwarder_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let token = extract_token_from_headers(&headers);
    ws.on_upgrade(move |socket| handle_forwarder_socket(socket, state, token))
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

fn created_reader_ips_for_logging<'a>(
    requested_reader_ips: &'a [String],
    stream_map: &HashMap<String, Uuid>,
) -> Vec<&'a str> {
    requested_reader_ips
        .iter()
        .filter(|reader_ip| stream_map.contains_key(reader_ip.as_str()))
        .map(String::as_str)
        .collect()
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
    if !is_path_safe_forwarder_id(&device_id) {
        send_ws_error(
            &mut socket,
            error_codes::INVALID_TOKEN,
            "forwarder id from token is not path-safe",
            false,
        )
        .await;
        return;
    }
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
    state.logger.log(format!("forwarder {device_id} connected"));
    let session_id = Uuid::new_v4().to_string();

    let hello = match recv_text_with_timeout(&mut socket, SESSION_TIMEOUT).await {
        Ok(text) => match serde_json::from_str::<WsMessage>(&text) {
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
        Err(()) => {
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
    if let Err(e) =
        update_forwarder_display_name(&state.pool, &device_id, current_display_name.as_deref())
            .await
    {
        error!(
            device_id = %device_id,
            error = %e,
            "failed to update forwarder display name"
        );
    }
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
    for reader_ip in created_reader_ips_for_logging(&hello.reader_ips, &stream_map) {
        state
            .logger
            .log(format!("stream created: {device_id}/{reader_ip}"));
    }

    let initial_display_name_patch = match &current_display_name {
        Some(name) => OptionalStringPatch::Set(name.clone()),
        None => OptionalStringPatch::Clear,
    };
    let initial_stream_ids = match fetch_stream_ids_by_forwarder(&state.pool, &device_id).await {
        Ok(ids) => ids,
        Err(e) => {
            error!(
                device_id = %device_id,
                error = %e,
                "failed to list forwarder streams for initial display-name update"
            );
            stream_map.values().copied().collect()
        }
    };
    for sid in initial_stream_ids {
        let _ = state.dashboard_tx.send(DashboardEvent::StreamUpdated {
            stream_id: sid,
            online: None,
            stream_epoch: None,
            display_alias: None,
            forwarder_display_name: Some(initial_display_name_patch.clone()),
        });
    }

    if !send_heartbeat(&mut socket, &session_id, &device_id).await {
        state.unregister_forwarder(&device_id).await;
        return;
    }

    let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<ForwarderCommand>(8);
    {
        let mut senders = state.forwarder_command_senders.write().await;
        senders.insert(device_id.clone(), cmd_tx);
    }

    let mut heartbeat_interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    heartbeat_interval.tick().await;

    let mut pending_config_gets: HashMap<
        String,
        (
            Instant,
            tokio::sync::oneshot::Sender<ForwarderProxyReply<rt_protocol::ConfigGetResponse>>,
        ),
    > = HashMap::new();
    let mut pending_config_sets: HashMap<
        String,
        (
            Instant,
            tokio::sync::oneshot::Sender<ForwarderProxyReply<rt_protocol::ConfigSetResponse>>,
        ),
    > = HashMap::new();
    let mut pending_restarts: HashMap<
        String,
        (
            Instant,
            tokio::sync::oneshot::Sender<ForwarderProxyReply<rt_protocol::RestartResponse>>,
        ),
    > = HashMap::new();

    loop {
        expire_pending_requests(&mut pending_config_gets, FORWARDER_COMMAND_TIMEOUT);
        expire_pending_requests(&mut pending_config_sets, FORWARDER_COMMAND_TIMEOUT);
        expire_pending_requests(&mut pending_restarts, FORWARDER_COMMAND_TIMEOUT);

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
                                let previous_display_name = current_display_name.clone();
                                current_display_name = new_hello.display_name.clone();
                                if let Err(e) = update_forwarder_display_name(
                                    &state.pool,
                                    &device_id,
                                    current_display_name.as_deref(),
                                )
                                .await
                                {
                                    error!(
                                        device_id = %device_id,
                                        error = %e,
                                        "failed to update forwarder display name"
                                    );
                                }
                                if previous_display_name != current_display_name {
                                    let display_name_patch = match &current_display_name {
                                        Some(name) => OptionalStringPatch::Set(name.clone()),
                                        None => OptionalStringPatch::Clear,
                                    };
                                    let stream_ids = match fetch_stream_ids_by_forwarder(
                                        &state.pool,
                                        &device_id,
                                    )
                                    .await
                                    {
                                        Ok(ids) => ids,
                                        Err(e) => {
                                            error!(
                                                device_id = %device_id,
                                                error = %e,
                                                "failed to list forwarder streams for display-name update"
                                            );
                                            stream_map.values().copied().collect()
                                        }
                                    };
                                    for sid in stream_ids {
                                        let _ = state.dashboard_tx.send(DashboardEvent::StreamUpdated {
                                            stream_id: sid,
                                            online: None,
                                            stream_epoch: None,
                                            display_alias: None,
                                            forwarder_display_name: Some(display_name_patch.clone()),
                                        });
                                    }
                                }
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
                                            state.logger.log(format!("stream created: {device_id}/{reader_ip}"));
                                        }
                                    }
                                }
                                if !send_heartbeat(&mut socket, &session_id, &device_id).await { break; }
                            }
                            Ok(WsMessage::Heartbeat(_)) => {}
                            Ok(WsMessage::ConfigGetResponse(resp)) => {
                                if let Some((_, reply)) = pending_config_gets.remove(&resp.request_id) {
                                    let _ = reply.send(ForwarderProxyReply::Response(resp));
                                }
                            }
                            Ok(WsMessage::ConfigSetResponse(resp)) => {
                                if let Some((_, reply)) = pending_config_sets.remove(&resp.request_id) {
                                    let _ = reply.send(ForwarderProxyReply::Response(resp));
                                }
                            }
                            Ok(WsMessage::RestartResponse(resp)) => {
                                if let Some((_, reply)) = pending_restarts.remove(&resp.request_id) {
                                    let _ = reply.send(ForwarderProxyReply::Response(resp));
                                }
                            }
                            Ok(_) => { warn!(device_id = %device_id, "unexpected message kind"); }
                            Err(e) => { send_ws_error(&mut socket, error_codes::PROTOCOL_ERROR, &format!("invalid JSON: {}", e), false).await; break; }
                        }
                    }
                    Ok(Some(Ok(Message::Ping(data)))) => { let _ = socket.send(Message::Pong(data)).await; }
                    Ok(Some(Ok(Message::Close(_)))) | Ok(None) => { state.logger.log(format!("forwarder {device_id} disconnected")); break; }
                    Err(_) => { state.logger.log_at(rt_ui_log::UiLogLevel::Warn, format!("forwarder {device_id} session timeout")); break; }
                    Ok(Some(Err(e))) => { state.logger.log_at(rt_ui_log::UiLogLevel::Warn, format!("forwarder {device_id} WS error: {e}")); break; }
                    Ok(Some(Ok(_))) => {}
                }
            }
            _ = heartbeat_interval.tick() => {
                expire_pending_requests(&mut pending_config_gets, FORWARDER_COMMAND_TIMEOUT);
                expire_pending_requests(&mut pending_config_sets, FORWARDER_COMMAND_TIMEOUT);
                expire_pending_requests(&mut pending_restarts, FORWARDER_COMMAND_TIMEOUT);
                if !send_heartbeat(&mut socket, &session_id, &device_id).await { break; }
            }
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    ForwarderCommand::EpochReset(epoch_cmd) => {
                        let msg = WsMessage::EpochResetCommand(epoch_cmd);
                        if let Ok(json) = serde_json::to_string(&msg) {
                            if socket.send(Message::Text(json)).await.is_err() { break; }
                        }
                    }
                    ForwarderCommand::ConfigGet { request_id, reply } => {
                        let msg = WsMessage::ConfigGetRequest(rt_protocol::ConfigGetRequest {
                            request_id: request_id.clone(),
                        });
                        if let Ok(json) = serde_json::to_string(&msg) {
                            if socket.send(Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                        pending_config_gets.insert(request_id, (Instant::now(), reply));
                    }
                    ForwarderCommand::ConfigSet { request_id, section, payload, reply } => {
                        let msg = WsMessage::ConfigSetRequest(rt_protocol::ConfigSetRequest {
                            request_id: request_id.clone(),
                            section,
                            payload,
                        });
                        if let Ok(json) = serde_json::to_string(&msg) {
                            if socket.send(Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                        pending_config_sets.insert(request_id, (Instant::now(), reply));
                    }
                    ForwarderCommand::Restart { request_id, reply } => {
                        let msg = WsMessage::RestartRequest(rt_protocol::RestartRequest {
                            request_id: request_id.clone(),
                        });
                        if let Ok(json) = serde_json::to_string(&msg) {
                            if socket.send(Message::Text(json)).await.is_err() {
                                break;
                            }
                        }
                        pending_restarts.insert(request_id, (Instant::now(), reply));
                    }
                }
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
            forwarder_display_name: None,
        });
    }
    {
        let mut senders = state.forwarder_command_senders.write().await;
        senders.remove(&device_id);
    }
    state.unregister_forwarder(&device_id).await;
    info!(device_id = %device_id, "forwarder session ended");
    state
        .logger
        .log(format!("forwarder {device_id} session ended"));
}

fn expire_pending_requests<T>(
    pending: &mut HashMap<
        String,
        (
            Instant,
            tokio::sync::oneshot::Sender<ForwarderProxyReply<T>>,
        ),
    >,
    timeout: Duration,
) {
    let now = Instant::now();
    let expired: Vec<String> = pending
        .iter()
        .filter_map(|(request_id, (started_at, _))| {
            if now.duration_since(*started_at) > timeout {
                Some(request_id.clone())
            } else {
                None
            }
        })
        .collect();

    for request_id in expired {
        if let Some((_, reply)) = pending.remove(&request_id) {
            let _ = reply.send(ForwarderProxyReply::Timeout);
        }
    }
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
                state.logger.log_at(
                    rt_ui_log::UiLogLevel::Warn,
                    format!(
                        "integrity conflict: {}/{} epoch={} seq={} — payload mismatch, keeping original",
                        device_id, event.reader_ip, event.stream_epoch, event.seq,
                    ),
                );
                let entry = high_water
                    .entry((event.reader_ip.clone(), event.stream_epoch))
                    .or_insert(0);
                if event.seq > *entry {
                    *entry = event.seq;
                }
            }
        }
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
            forwarder_display_name: None,
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
            last_tag_id: m.last_tag_id.clone(),
            last_reader_timestamp: m.last_reader_timestamp.clone(),
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn created_reader_ips_only_includes_successful_upserts_in_requested_order() {
        let requested = vec![
            "10.0.0.1:10000".to_owned(),
            "10.0.0.2:10000".to_owned(),
            "10.0.0.3:10000".to_owned(),
        ];
        let mut stream_map = HashMap::new();
        stream_map.insert("10.0.0.1:10000".to_owned(), Uuid::new_v4());
        stream_map.insert("10.0.0.3:10000".to_owned(), Uuid::new_v4());

        let created = created_reader_ips_for_logging(&requested, &stream_map);

        assert_eq!(created, vec!["10.0.0.1:10000", "10.0.0.3:10000"]);
    }
}
