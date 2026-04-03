use crate::{
    announcer::{AnnouncerEvent, AnnouncerInputEvent},
    auth::validate_token,
    dashboard_events::{DashboardEvent, OptionalStringPatch},
    repo::{
        announcer_config,
        events::{
            IngestResult, count_unique_chips, fetch_stream_ids_by_forwarder, fetch_stream_metrics,
            fetch_stream_snapshot, set_reader_connected, set_stream_online,
            update_forwarder_display_name, upsert_event, upsert_stream,
        },
        races::lookup_stream_chip_participant,
    },
    state::{AppState, CachedReaderState, ForwarderCommand, ForwarderProxyReply},
    ws_common::{
        extract_token_from_headers, recv_text_with_timeout, send_heartbeat, send_ws_error,
    },
};
use axum::{
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::IntoResponse,
};
use rt_protocol::{AckEntry, ForwarderAck, ReadEvent, WsMessage, error_codes};
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
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
    match fetch_stream_snapshot(&state.pool, stream_id).await {
        Ok(Some(stream)) => {
            let _ = state.dashboard_tx.send(DashboardEvent::StreamCreated {
                stream_id: stream.stream_id,
                forwarder_id: stream.forwarder_id,
                reader_ip: stream.reader_ip,
                display_alias: stream.display_alias,
                forwarder_display_name: stream.forwarder_display_name,
                online: stream.online,
                reader_connected: stream.reader_connected,
                stream_epoch: stream.stream_epoch,
                created_at: stream.created_at.to_rfc3339(),
            });
        }
        Ok(None) => {
            warn!(stream_id = %stream_id, "stream not found when publishing StreamCreated");
        }
        Err(e) => {
            error!(stream_id = %stream_id, error = %e, "failed to fetch stream snapshot for dashboard");
        }
    }
}

async fn publish_forwarder_metrics_updated(state: &AppState, forwarder_id: &str) {
    match crate::http::forwarders_list::fetch_forwarder_metrics(&state.pool, forwarder_id).await {
        Ok(Some(metrics)) => {
            let _ = state
                .dashboard_tx
                .send(DashboardEvent::ForwarderMetricsUpdated {
                    forwarder_id: metrics.forwarder_id,
                    unique_chips: metrics.unique_chips,
                    total_reads: metrics.total_reads,
                    last_read_at: metrics.last_read_at.map(|ts| ts.to_rfc3339()),
                });
        }
        Ok(None) => {
            warn!(
                forwarder_id = %forwarder_id,
                "forwarder metrics update requested for unknown forwarder"
            );
        }
        Err(e) => {
            error!(
                forwarder_id = %forwarder_id,
                error = %e,
                "failed to fetch forwarder metrics for dashboard update"
            );
        }
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

fn parse_chip_id_from_raw_frame(raw_frame: &[u8]) -> Option<String> {
    let trimmed = raw_frame
        .strip_suffix(b"\r\n")
        .or_else(|| raw_frame.strip_suffix(b"\n"))
        .unwrap_or(raw_frame);
    let raw = std::str::from_utf8(trimmed).ok()?;
    let chip = ipico_core::read::ChipRead::try_from(raw).ok()?;
    Some(chip.tag_id)
}

fn display_name_change_log_entry(
    device_id: &str,
    current_display_name: Option<&str>,
    persisted: bool,
) -> (rt_ui_log::UiLogLevel, String) {
    if !persisted {
        return (
            rt_ui_log::UiLogLevel::Warn,
            format!("forwarder \"{device_id}\" display name changed but failed to persist"),
        );
    }

    match current_display_name {
        Some(name) => (
            rt_ui_log::UiLogLevel::Info,
            format!("forwarder \"{device_id}\" display name set to \"{name}\""),
        ),
        None => (
            rt_ui_log::UiLogLevel::Info,
            format!("forwarder \"{device_id}\" display name cleared"),
        ),
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
        Ok(Some(c)) => c,
        Ok(None) => {
            send_ws_error(
                &mut socket,
                error_codes::INVALID_TOKEN,
                "unknown or revoked token",
                false,
            )
            .await;
            return;
        }
        Err(e) => {
            error!(error = %e, "database error during token validation");
            send_ws_error(
                &mut socket,
                error_codes::INTERNAL_ERROR,
                "internal server error",
                true,
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
                warn!(error = %e, "invalid JSON in forwarder hello");
                send_ws_error(
                    &mut socket,
                    error_codes::PROTOCOL_ERROR,
                    "invalid JSON in hello message",
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
        match upsert_stream(
            &state.pool,
            &device_id,
            reader_ip,
            current_display_name.as_deref(),
        )
        .await
        {
            Ok(sid) => {
                stream_map.insert(reader_ip.clone(), sid);
                if let Err(e) = set_stream_online(&state.pool, sid, true).await {
                    error!(
                        device_id = %device_id,
                        reader_ip = %reader_ip,
                        stream_id = %sid,
                        error = %e,
                        "failed to mark stream online during hello"
                    );
                }
                state.get_or_create_broadcast(sid).await;
            }
            Err(e) => {
                error!(
                    device_id = %device_id,
                    reader_ip = %reader_ip,
                    error = %e,
                    "failed to upsert stream"
                );
            }
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
            reader_connected: None,
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
    let mut pending_reader_controls: HashMap<
        String,
        (
            Instant,
            tokio::sync::oneshot::Sender<ForwarderProxyReply<rt_protocol::ReaderControlResponse>>,
        ),
    > = HashMap::new();

    loop {
        expire_pending_requests(&mut pending_config_gets, FORWARDER_COMMAND_TIMEOUT);
        expire_pending_requests(&mut pending_config_sets, FORWARDER_COMMAND_TIMEOUT);
        expire_pending_requests(&mut pending_restarts, FORWARDER_COMMAND_TIMEOUT);
        expire_pending_requests(&mut pending_reader_controls, FORWARDER_COMMAND_TIMEOUT);

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
                                let display_name_persisted = match update_forwarder_display_name(
                                    &state.pool,
                                    &device_id,
                                    current_display_name.as_deref(),
                                )
                                .await
                                {
                                    Ok(_) => true,
                                    Err(e) => {
                                        error!(
                                            device_id = %device_id,
                                            error = %e,
                                            "failed to update forwarder display name"
                                        );
                                        false
                                    }
                                };
                                if previous_display_name != current_display_name {
                                    let (level, msg) = display_name_change_log_entry(
                                        &device_id,
                                        current_display_name.as_deref(),
                                        display_name_persisted,
                                    );
                                    state.logger.log_at(level, msg);
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
                                            reader_connected: None,
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
                                        && !stream_map.contains_key(reader_ip)
                                    {
                                        stream_map.insert(reader_ip.clone(), sid);
                                        if let Err(e) = set_stream_online(&state.pool, sid, true).await {
                                            error!(
                                                device_id = %device_id,
                                                reader_ip = %reader_ip,
                                                stream_id = %sid,
                                                error = %e,
                                                "failed to mark stream online during re-hello"
                                            );
                                        }
                                        state.get_or_create_broadcast(sid).await;
                                        publish_stream_created(&state, sid).await;
                                        state.logger.log(format!("stream created: {device_id}/{reader_ip}"));
                                    }
                                }
                                if !send_heartbeat(&mut socket, &session_id, &device_id).await { break; }
                            }
                            Ok(WsMessage::Heartbeat(_)) => {}
                            Ok(WsMessage::ReaderStatusUpdate(update)) => {
                                if let Some(sid) = stream_map.get(&update.reader_ip) {
                                    let applied = match set_reader_connected(&state.pool, *sid, update.connected).await {
                                        Ok(applied) => applied,
                                        Err(e) => {
                                            error!(
                                                device_id = %device_id,
                                                reader_ip = %update.reader_ip,
                                                error = %e,
                                                "failed to persist reader_connected; SSE/broadcast will diverge from HTTP API until next update"
                                            );
                                            // On DB error, still broadcast for best-effort real-time updates
                                            true
                                        }
                                    };
                                    if !applied {
                                        warn!(
                                            device_id = %device_id,
                                            reader_ip = %update.reader_ip,
                                            stream_id = %sid,
                                            "reader_connected=true rejected: stream is offline"
                                        );
                                    }
                                    if applied {
                                        let _ = state.dashboard_tx.send(DashboardEvent::StreamUpdated {
                                            stream_id: *sid,
                                            online: None,
                                            reader_connected: Some(update.connected),
                                            stream_epoch: None,
                                            display_alias: None,
                                            forwarder_display_name: None,
                                        });
                                        // Forward ReaderStatusChanged to connected receivers.
                                        // The per-stream broadcast is typed ReadEvent, so we
                                        // encode the status change as a sentinel ReadEvent that
                                        // the receiver handler will detect and forward directly.
                                        let changed = rt_protocol::ReaderStatusChanged {
                                            stream_id: *sid,
                                            reader_ip: update.reader_ip.clone(),
                                            connected: update.connected,
                                        };
                                        match serde_json::to_string(&WsMessage::ReaderStatusChanged(changed)) {
                                            Ok(json) => {
                                                let tx = state.get_or_create_broadcast(*sid).await;
                                                let _ = tx.send(ReadEvent {
                                                    forwarder_id: device_id.clone(),
                                                    reader_ip: update.reader_ip.clone(),
                                                    // stream_epoch and seq are unused for sentinel events; set to 0 as placeholders.
                                                    stream_epoch: 0,
                                                    seq: 0,
                                                    reader_timestamp: String::new(),
                                                    raw_frame: json.into_bytes(),
                                                    read_type: rt_protocol::READER_STATUS_CHANGED_READ_TYPE.to_owned(),
                                                });
                                            }
                                            Err(e) => {
                                                error!(
                                                    device_id = %device_id,
                                                    reader_ip = %update.reader_ip,
                                                    error = %e,
                                                    "failed to serialize ReaderStatusChanged for broadcast"
                                                );
                                            }
                                        }
                                    }
                                } else {
                                    warn!(
                                        device_id = %device_id,
                                        reader_ip = %update.reader_ip,
                                        "reader_status_update for unknown reader_ip"
                                    );
                                }
                            }
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
                            Ok(WsMessage::ReaderControlResponse(resp)) => {
                                if let Some((_, reply)) = pending_reader_controls.remove(&resp.request_id) {
                                    if reply.send(ForwarderProxyReply::Response(resp)).is_err() {
                                        warn!(device_id = %device_id, "reader control reply dropped (HTTP handler already timed out)");
                                    }
                                } else {
                                    warn!(device_id = %device_id, request_id = %resp.request_id, "reader control response for unknown request_id");
                                }
                            }
                            Ok(WsMessage::ReaderInfoUpdate(update)) => {
                                let key = crate::state::reader_cache_key(&device_id, &update.reader_ip);
                                // Merge: carry forward existing reader_info when the update is state-only
                                let merged_info = if update.reader_info.is_some() {
                                    update.reader_info.clone()
                                } else {
                                    state.reader_states.read().await
                                        .get(&key)
                                        .and_then(|c| c.reader_info.clone())
                                };
                                let cached = CachedReaderState {
                                    forwarder_id: device_id.clone(),
                                    reader_ip: update.reader_ip.clone(),
                                    state: update.state,
                                    reader_info: merged_info.clone(),
                                };
                                state.reader_states.write().await.insert(key, cached);

                                // Tunnel to receivers via sentinel broadcast (before dashboard send moves fields)
                                if let Some(stream_id) = stream_map.get(&update.reader_ip) {
                                    let receiver_update = rt_protocol::ReceiverReaderInfoUpdate {
                                        stream_id: *stream_id,
                                        reader_ip: update.reader_ip.clone(),
                                        state: update.state,
                                        reader_info: merged_info.clone(),
                                    };
                                    match serde_json::to_string(
                                        &rt_protocol::WsMessage::ReceiverReaderInfoUpdate(receiver_update),
                                    ) {
                                        Ok(json) => {
                                            let tx = state.get_or_create_broadcast(*stream_id).await;
                                            let _ = tx.send(rt_protocol::ReadEvent {
                                                forwarder_id: device_id.clone(),
                                                reader_ip: update.reader_ip.clone(),
                                                stream_epoch: 0,
                                                seq: 0,
                                                reader_timestamp: String::new(),
                                                raw_frame: json.into_bytes(),
                                                read_type: rt_protocol::READER_INFO_UPDATED_READ_TYPE.to_owned(),
                                            });
                                        }
                                        Err(e) => {
                                            error!(
                                                device_id = %device_id,
                                                reader_ip = %update.reader_ip,
                                                error = %e,
                                                "failed to serialize ReceiverReaderInfoUpdate for broadcast"
                                            );
                                        }
                                    }
                                } else {
                                    warn!(
                                        device_id = %device_id,
                                        reader_ip = %update.reader_ip,
                                        "reader_info_update for unknown reader_ip"
                                    );
                                }

                                let _ = state.dashboard_tx.send(DashboardEvent::ReaderInfoUpdated {
                                    forwarder_id: device_id.clone(),
                                    reader_ip: update.reader_ip,
                                    state: update.state,
                                    reader_info: merged_info,
                                });
                            }
                            Ok(WsMessage::ReaderDownloadProgress(progress)) => {
                                // Tunnel to receivers via sentinel broadcast (before dashboard send moves progress)
                                if let Some(stream_id) = stream_map.get(&progress.reader_ip) {
                                    let receiver_progress = rt_protocol::ReceiverReaderDownloadProgress {
                                        stream_id: *stream_id,
                                        reader_ip: progress.reader_ip.clone(),
                                        state: progress.state,
                                        reads_received: progress.reads_received,
                                        progress: progress.progress,
                                        total: progress.total,
                                        error: progress.error.clone(),
                                    };
                                    match serde_json::to_string(
                                        &rt_protocol::WsMessage::ReceiverReaderDownloadProgress(receiver_progress),
                                    ) {
                                        Ok(json) => {
                                            let tx = state.get_or_create_broadcast(*stream_id).await;
                                            let _ = tx.send(rt_protocol::ReadEvent {
                                                forwarder_id: device_id.clone(),
                                                reader_ip: progress.reader_ip.clone(),
                                                stream_epoch: 0,
                                                seq: 0,
                                                reader_timestamp: String::new(),
                                                raw_frame: json.into_bytes(),
                                                read_type: rt_protocol::READER_DOWNLOAD_PROGRESS_READ_TYPE.to_owned(),
                                            });
                                        }
                                        Err(e) => {
                                            error!(
                                                device_id = %device_id,
                                                reader_ip = %progress.reader_ip,
                                                error = %e,
                                                "failed to serialize ReceiverReaderDownloadProgress for broadcast"
                                            );
                                        }
                                    }
                                } else {
                                    warn!(
                                        device_id = %device_id,
                                        reader_ip = %progress.reader_ip,
                                        "reader_download_progress for unknown reader_ip"
                                    );
                                }

                                let _ = state.dashboard_tx.send(DashboardEvent::ReaderDownloadProgress {
                                    forwarder_id: device_id.clone(),
                                    progress,
                                });
                            }
                            Ok(WsMessage::ForwarderUpsStatus(ups)) => {
                                // 1. Check power_plugged transition for event logging
                                let prev_plugged = {
                                    let cache = state.forwarder_ups_cache.read().await;
                                    cache.get(&device_id).and_then(|c| c.status.as_ref().map(|s| s.power_plugged))
                                };

                                // 2. Update cache
                                {
                                    let mut cache = state.forwarder_ups_cache.write().await;
                                    cache.insert(device_id.clone(), crate::state::CachedUpsState {
                                        available: ups.available,
                                        status: ups.status.clone(),
                                    });
                                }

                                // 3. Log power_plugged transitions to DB
                                if let Some(ref status) = ups.status
                                    && let Some(was_plugged) = prev_plugged
                                {
                                    if was_plugged && !status.power_plugged {
                                        let _ = sqlx::query(
                                            "INSERT INTO forwarder_ups_events (forwarder_id, event_type, battery_percent) VALUES ($1, 'power_lost', $2)"
                                        )
                                        .bind(&device_id)
                                        .bind(status.battery_percent as i16)
                                        .execute(&state.pool)
                                        .await;
                                    } else if !was_plugged && status.power_plugged {
                                        let _ = sqlx::query(
                                            "INSERT INTO forwarder_ups_events (forwarder_id, event_type, battery_percent) VALUES ($1, 'power_restored', $2)"
                                        )
                                        .bind(&device_id)
                                        .bind(status.battery_percent as i16)
                                        .execute(&state.pool)
                                        .await;
                                    }
                                }

                                // 4. Emit dashboard SSE event
                                let _ = state.dashboard_tx.send(DashboardEvent::ForwarderUpsUpdated {
                                    forwarder_id: device_id.clone(),
                                    available: ups.available,
                                    status: ups.status.clone(),
                                });

                            }
                            Ok(_) => { warn!(device_id = %device_id, "unexpected message kind"); }
                            Err(e) => { warn!(device_id = %device_id, error = %e, "invalid JSON in forwarder session message"); send_ws_error(&mut socket, error_codes::PROTOCOL_ERROR, "invalid JSON in message", false).await; break; }
                        }
                    }
                    Ok(Some(Ok(Message::Ping(data)))) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            warn!(device_id = %device_id, "failed to send Pong to forwarder, connection likely dead");
                            break;
                        }
                    }
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
                expire_pending_requests(&mut pending_reader_controls, FORWARDER_COMMAND_TIMEOUT);
                if !send_heartbeat(&mut socket, &session_id, &device_id).await { break; }
            }
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    ForwarderCommand::EpochReset(epoch_cmd) => {
                        let msg = WsMessage::EpochResetCommand(epoch_cmd);
                        if let Ok(json) = serde_json::to_string(&msg)
                            && socket.send(Message::Text(json.into())).await.is_err()
                        {
                            break;
                        }
                    }
                    ForwarderCommand::ConfigGet { request_id, reply } => {
                        let msg = WsMessage::ConfigGetRequest(rt_protocol::ConfigGetRequest {
                            request_id: request_id.clone(),
                        });
                        match serde_json::to_string(&msg) {
                            Ok(json) => {
                                if socket.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                                pending_config_gets.insert(request_id, (Instant::now(), reply));
                            }
                            Err(e) => {
                                error!(device_id = %device_id, error = %e, "failed to serialize config get request");
                                let _ = reply.send(ForwarderProxyReply::InternalError(
                                    format!("failed to serialize config get request: {}", e),
                                ));
                            }
                        }
                    }
                    ForwarderCommand::ConfigSet { request_id, section, payload, reply } => {
                        let msg = WsMessage::ConfigSetRequest(rt_protocol::ConfigSetRequest {
                            request_id: request_id.clone(),
                            section,
                            payload,
                        });
                        match serde_json::to_string(&msg) {
                            Ok(json) => {
                                if socket.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                                pending_config_sets.insert(request_id, (Instant::now(), reply));
                            }
                            Err(e) => {
                                error!(device_id = %device_id, error = %e, "failed to serialize config set request");
                                let _ = reply.send(ForwarderProxyReply::InternalError(
                                    format!("failed to serialize config set request: {}", e),
                                ));
                            }
                        }
                    }
                    ForwarderCommand::Restart { request_id, reply } => {
                        let msg = WsMessage::RestartRequest(rt_protocol::RestartRequest {
                            request_id: request_id.clone(),
                        });
                        match serde_json::to_string(&msg) {
                            Ok(json) => {
                                if socket.send(Message::Text(json.into())).await.is_err() {
                                    break;
                                }
                                pending_restarts.insert(request_id, (Instant::now(), reply));
                            }
                            Err(e) => {
                                error!(device_id = %device_id, error = %e, "failed to serialize restart request");
                                let _ = reply.send(ForwarderProxyReply::InternalError(
                                    format!("failed to serialize restart request: {}", e),
                                ));
                            }
                        }
                    }
                    ForwarderCommand::ReaderControl { request_id, reader_ip, action, reply } => {
                        let msg = WsMessage::ReaderControlRequest(rt_protocol::ReaderControlRequest {
                            request_id: request_id.clone(),
                            reader_ip,
                            action,
                        });
                        match serde_json::to_string(&msg) {
                            Ok(json) => {
                                if let Err(e) = socket.send(Message::Text(json.into())).await {
                                    warn!(error = %e, "failed to send reader control request");
                                    let _ = reply.send(ForwarderProxyReply::InternalError(
                                        format!("failed to send to forwarder: {}", e),
                                    ));
                                } else {
                                    pending_reader_controls.insert(request_id, (Instant::now(), reply));
                                }
                            }
                            Err(e) => {
                                let err_msg = format!("failed to serialize reader control request: {}", e);
                                error!(device_id = %device_id, error = %e, "failed to serialize reader control request");
                                let _ = reply.send(ForwarderProxyReply::InternalError(err_msg));
                            }
                        }
                    }
                    ForwarderCommand::ReaderControlFireAndForget { reader_ip, action } => {
                        let msg = WsMessage::ReaderControlRequest(rt_protocol::ReaderControlRequest {
                            request_id: uuid::Uuid::new_v4().to_string(),
                            reader_ip,
                            action,
                        });
                        match serde_json::to_string(&msg) {
                            Ok(json) => {
                                if let Err(e) = socket.send(Message::Text(json.into())).await {
                                    warn!(error = %e, "failed to send fire-and-forget reader control");
                                }
                            }
                            Err(e) => {
                                error!(device_id = %device_id, error = %e, "failed to serialize fire-and-forget reader control request");
                            }
                        }
                    }
                }
            }
        }
    }

    for sid in stream_map.values() {
        if let Err(e) = set_stream_online(&state.pool, *sid, false).await {
            error!(
                stream_id = %sid,
                error = %e,
                "failed to mark stream offline during disconnect cleanup"
            );
        }
        let _ = state.dashboard_tx.send(DashboardEvent::StreamUpdated {
            stream_id: *sid,
            online: Some(false),
            reader_connected: Some(false),
            stream_epoch: None,
            display_alias: None,
            forwarder_display_name: None,
        });
    }
    {
        let mut cache = state.reader_states.write().await;
        let keys_to_remove: Vec<String> = cache
            .keys()
            .filter(|k| k.starts_with(&format!("{}:", device_id)))
            .cloned()
            .collect();
        for key in &keys_to_remove {
            if let Some(cached) = cache.remove(key) {
                let _ = state.dashboard_tx.send(DashboardEvent::ReaderInfoUpdated {
                    forwarder_id: device_id.clone(),
                    reader_ip: cached.reader_ip,
                    state: rt_protocol::ReaderConnectionState::Disconnected,
                    reader_info: None,
                });
            }
        }
    }
    // Clear UPS cache for this forwarder
    {
        state.forwarder_ups_cache.write().await.remove(&device_id);
        let _ = state
            .dashboard_tx
            .send(DashboardEvent::ForwarderUpsUpdated {
                forwarder_id: device_id.clone(),
                available: false,
                status: None,
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
            warn!(request_id = %request_id, "proxy request timed out");
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
    let mut high_water: HashMap<(String, i64), i64> = HashMap::new();
    let mut epoch_transitions: HashMap<Uuid, i64> = HashMap::new();
    let config = announcer_config::get_config(&state.pool).await.ok();
    let now = chrono::Utc::now();
    let (announcer_enabled, announcer_selected_streams, announcer_max_list_size) =
        if let Some(config) = config {
            let not_expired = config.enabled_until.map(|ts| ts > now).unwrap_or(true);
            (
                config.enabled && not_expired,
                config
                    .selected_stream_ids
                    .into_iter()
                    .collect::<HashSet<Uuid>>(),
                usize::try_from(config.max_list_size.max(1)).unwrap_or(25),
            )
        } else {
            (false, HashSet::new(), 25)
        };

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
            if let Err(e) = set_stream_online(&state.pool, sid, true).await {
                error!(
                    device_id = %device_id,
                    reader_ip = %event.reader_ip,
                    stream_id = %sid,
                    error = %e,
                    "failed to mark stream online during event batch"
                );
            }
            state.get_or_create_broadcast(sid).await;
            sid
        };

        let result = upsert_event(
            &state.pool,
            stream_id,
            event.stream_epoch,
            event.seq,
            &event.reader_timestamp,
            &event.raw_frame,
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

                if announcer_enabled
                    && announcer_selected_streams.contains(&stream_id)
                    && let Some(chip_id) = parse_chip_id_from_raw_frame(&event.raw_frame)
                {
                    let participant = match lookup_stream_chip_participant(
                        &state.pool,
                        stream_id,
                        &chip_id,
                    )
                    .await
                    {
                        Ok(found) => found,
                        Err(err) => {
                            warn!(
                                stream_id = %stream_id,
                                error = %err,
                                "failed to resolve announcer participant; using unknown"
                            );
                            None
                        }
                    };

                    let (display_name, bib) = match participant {
                        Some(participant) => match (
                            participant.first_name.as_deref(),
                            participant.last_name.as_deref(),
                        ) {
                            (Some(first), Some(last)) => {
                                (format!("{first} {last}").trim().to_owned(), participant.bib)
                            }
                            _ => ("Unknown".to_owned(), participant.bib),
                        },
                        None => ("Unknown".to_owned(), None),
                    };

                    let mut runtime = state.announcer_runtime.write().await;
                    if let Some(delta) = runtime.ingest(
                        AnnouncerInputEvent {
                            stream_id,
                            seq: event.seq,
                            chip_id,
                            bib,
                            display_name,
                            reader_timestamp: Some(event.reader_timestamp.clone()),
                            received_at: chrono::Utc::now(),
                        },
                        announcer_max_list_size,
                    ) {
                        let _ = state.announcer_tx.send(AnnouncerEvent::Update(delta));
                    }
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
        socket.send(Message::Text(json.into())).await?;
    }

    if epoch_transitions
        .keys()
        .any(|stream_id| announcer_selected_streams.contains(stream_id))
    {
        state.reset_announcer_runtime().await;
    }

    for (stream_id, new_epoch) in epoch_transitions {
        let _ = state.dashboard_tx.send(DashboardEvent::StreamUpdated {
            stream_id,
            online: None,
            reader_connected: None,
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

    publish_forwarder_metrics_updated(state, device_id).await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_change_log_entry_uses_info_on_persist_success() {
        let (level, msg) = display_name_change_log_entry("fwd-1", Some("Start"), true);
        assert_eq!(level, rt_ui_log::UiLogLevel::Info);
        assert_eq!(msg, "forwarder \"fwd-1\" display name set to \"Start\"");
    }

    #[test]
    fn display_name_change_log_entry_uses_warn_on_persist_failure() {
        let (level, msg) = display_name_change_log_entry("fwd-1", Some("Start"), false);
        assert_eq!(level, rt_ui_log::UiLogLevel::Warn);
        assert_eq!(
            msg,
            "forwarder \"fwd-1\" display name changed but failed to persist"
        );
    }

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
