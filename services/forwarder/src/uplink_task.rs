// uplink_task: WebSocket uplink orchestration loop — replay, send batches, receive acks,
// and dispatch incoming control messages from the server.

use forwarder::config::ForwarderConfig;
use forwarder::replay::ReplayEngine;
use forwarder::status_http::{ConfigState, ReaderConnectionState, StatusServer, SubsystemStatus};
use forwarder::storage::journal::Journal;
use forwarder::ui_events::ForwarderUiEvent;
use forwarder::uplink::{SendBatchResult, UplinkConfig, UplinkError, UplinkSession};
use forwarder::uplink_replay::should_reconnect_after_replay_send;
use rt_protocol::{ReadEvent, WsMessage};
use rt_ui_log::UiLogLevel;
use std::sync::Arc;
use tokio::sync::{Mutex, Notify, watch};
use tokio::time::{Duration, sleep};
use tracing::{debug, info, warn};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(crate) fn chunk_for_replay(
    events: Vec<ReadEvent>,
    max_events_per_batch: u32,
) -> Vec<Vec<ReadEvent>> {
    let chunk_size = max_events_per_batch.max(1) as usize;
    events
        .chunks(chunk_size)
        .map(std::borrow::ToOwned::to_owned)
        .collect()
}

fn ui_to_protocol_connection_state(
    state: forwarder::ui_events::ReaderConnectionState,
) -> rt_protocol::ReaderConnectionState {
    match state {
        forwarder::ui_events::ReaderConnectionState::Connected => {
            rt_protocol::ReaderConnectionState::Connected
        }
        forwarder::ui_events::ReaderConnectionState::Connecting => {
            rt_protocol::ReaderConnectionState::Connecting
        }
        forwarder::ui_events::ReaderConnectionState::Disconnected => {
            rt_protocol::ReaderConnectionState::Disconnected
        }
    }
}

fn status_to_protocol_connection_state(
    state: ReaderConnectionState,
) -> rt_protocol::ReaderConnectionState {
    match state {
        ReaderConnectionState::Connected => rt_protocol::ReaderConnectionState::Connected,
        ReaderConnectionState::Connecting => rt_protocol::ReaderConnectionState::Connecting,
        ReaderConnectionState::Disconnected => rt_protocol::ReaderConnectionState::Disconnected,
    }
}

fn ipico_to_protocol_read_mode(mode: ipico_core::control::ReadMode) -> rt_protocol::ReadMode {
    match mode {
        ipico_core::control::ReadMode::Raw => rt_protocol::ReadMode::Raw,
        ipico_core::control::ReadMode::Event => rt_protocol::ReadMode::Event,
        ipico_core::control::ReadMode::FirstLastSeen => rt_protocol::ReadMode::FirstLastSeen,
    }
}

fn protocol_to_ipico_read_mode(mode: rt_protocol::ReadMode) -> ipico_core::control::ReadMode {
    match mode {
        rt_protocol::ReadMode::Raw => ipico_core::control::ReadMode::Raw,
        rt_protocol::ReadMode::Event => ipico_core::control::ReadMode::Event,
        rt_protocol::ReadMode::FirstLastSeen => ipico_core::control::ReadMode::FirstLastSeen,
    }
}

fn extract_error_message(err_json: String) -> String {
    serde_json::from_str::<serde_json::Value>(&err_json)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| e.as_str())
                .map(|s| s.to_owned())
        })
        .unwrap_or(err_json)
}

async fn process_ack(journal: &Mutex<Journal>, ack: &rt_protocol::ForwarderAck) {
    let mut j = journal.lock().await;
    for entry in &ack.entries {
        if let Err(e) = j.update_ack_cursor(&entry.reader_ip, entry.stream_epoch, entry.last_seq) {
            warn!(error = %e, "failed to update ack cursor");
        }
        if let Err(e) = j.prune_acked(&entry.reader_ip, 500) {
            warn!(error = %e, reader_ip = %entry.reader_ip, "journal prune failed after ack");
        }
    }
}

// ---------------------------------------------------------------------------
// Config message handler (used by uplink loop)
// ---------------------------------------------------------------------------

pub(crate) async fn handle_config_message(
    session: &mut UplinkSession,
    msg: WsMessage,
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<forwarder::ui_events::ForwarderUiEvent>,
) -> Result<(), UplinkError> {
    match msg {
        WsMessage::ConfigGetRequest(req) => {
            let response =
                match forwarder::status_http::read_config_json(config_state, subsystem).await {
                    Ok((config, restart_needed)) => {
                        WsMessage::ConfigGetResponse(rt_protocol::ConfigGetResponse {
                            request_id: req.request_id,
                            ok: true,
                            error: None,
                            config,
                            restart_needed,
                        })
                    }
                    Err((_code, err_json)) => {
                        let err_msg = extract_error_message(err_json);
                        WsMessage::ConfigGetResponse(rt_protocol::ConfigGetResponse {
                            request_id: req.request_id,
                            ok: false,
                            error: Some(err_msg),
                            config: serde_json::Value::Null,
                            restart_needed: false,
                        })
                    }
                };
            session.send_message(&response).await
        }
        WsMessage::ConfigSetRequest(req) => {
            let (ok, error, restart_needed, status_code) =
                match forwarder::status_http::apply_section_update(
                    &req.section,
                    &req.payload,
                    config_state,
                    subsystem,
                    ui_tx,
                    None,
                )
                .await
                {
                    Ok(()) => (true, None, subsystem.lock().await.restart_needed(), None),
                    Err((status, err_json)) => {
                        let err_msg = extract_error_message(err_json);
                        (
                            false,
                            Some(err_msg),
                            subsystem.lock().await.restart_needed(),
                            Some(status),
                        )
                    }
                };
            let response = WsMessage::ConfigSetResponse(rt_protocol::ConfigSetResponse {
                request_id: req.request_id,
                ok,
                error,
                restart_needed,
                status_code,
            });
            session.send_message(&response).await
        }
        other => {
            debug!(
                "handle_config_message called with non-config message: {:?}",
                std::mem::discriminant(&other)
            );
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------
// Restart message handler (used by uplink loop)
// ---------------------------------------------------------------------------

pub(crate) async fn handle_restart_message(
    session: &mut UplinkSession,
    req: rt_protocol::RestartRequest,
    restart_signal: &Arc<Notify>,
) -> Result<(), UplinkError> {
    let (ok, error) = if cfg!(unix) {
        restart_signal.notify_one();
        (true, None)
    } else {
        (
            false,
            Some("restart not supported on non-unix platforms".to_owned()),
        )
    };
    let response = WsMessage::RestartResponse(rt_protocol::RestartResponse {
        request_id: req.request_id,
        ok,
        error,
    });
    session.send_message(&response).await
}

// ---------------------------------------------------------------------------
// Reader control message handler (used by uplink loop)
// ---------------------------------------------------------------------------

/// Convert forwarder-internal `ReaderInfo` to the protocol's `rt_protocol::ReaderInfo`.
pub(crate) fn to_protocol_reader_info(
    info: &forwarder::reader_control::ReaderInfo,
) -> rt_protocol::ReaderInfo {
    rt_protocol::ReaderInfo {
        banner: info.banner.clone(),
        hardware: info.hardware.as_ref().map(|h| rt_protocol::HardwareInfo {
            fw_version: Some(h.fw_version.clone()),
            hw_code: Some(format!("0x{:02X}", h.hw_code)),
            reader_id: Some(format!("0x{:02X}", h.reader_id)),
        }),
        config: info.config.as_ref().map(|c| rt_protocol::Config3Info {
            mode: ipico_to_protocol_read_mode(c.mode),
            timeout: c.timeout,
        }),
        tto_enabled: info.tto_enabled,
        clock: info.clock.as_ref().map(|c| rt_protocol::ClockInfo {
            reader_clock: c.reader_clock.clone(),
            drift_ms: c.drift_ms,
        }),
        estimated_stored_reads: info.estimated_stored_reads,
        recording: info.recording,
        connect_failures: info.connect_failures,
    }
}

fn reader_control_response(
    request_id: String,
    reader_ip: String,
    success: bool,
    error: Option<String>,
    reader_info: Option<rt_protocol::ReaderInfo>,
) -> WsMessage {
    WsMessage::ReaderControlResponse(rt_protocol::ReaderControlResponse {
        request_id,
        reader_ip,
        success,
        error,
        reader_info,
    })
}

pub(crate) async fn handle_reader_control_message(
    session: &mut UplinkSession,
    req: rt_protocol::ReaderControlRequest,
    status: &StatusServer,
) -> Result<(), UplinkError> {
    use rt_protocol::ReaderControlAction;

    let request_id = req.request_id.clone();
    let reader_ip = req.reader_ip.clone();

    // Look up the ControlClient by reader_ip
    let client = {
        status
            .control_clients()
            .read()
            .unwrap_or_else(|e| {
                warn!("recovered from poisoned RwLock");
                e.into_inner()
            })
            .get(&reader_ip)
            .cloned()
    };

    let Some(client) = client else {
        let response = reader_control_response(
            request_id,
            reader_ip,
            false,
            Some("reader not connected".to_owned()),
            None,
        );
        return session.send_message(&response).await;
    };

    // Helper closures for cached info access via StatusServer
    let get_cached_info = |status: &StatusServer, ip: &str| {
        let status = status.clone();
        let ip = ip.to_owned();
        async move { status.get_reader_info(&ip).await.unwrap_or_default() }
    };

    let update_info =
        |status: &StatusServer, ip: &str, info: forwarder::reader_control::ReaderInfo| {
            let status = status.clone();
            let ip = ip.to_owned();
            async move {
                status
                    .update_reader_info_unless_disconnected(&ip, info)
                    .await;
            }
        };

    match req.action {
        ReaderControlAction::GetInfo | ReaderControlAction::Refresh => {
            let mut info = get_cached_info(status, &reader_ip).await;
            forwarder::reader_control::run_status_poll(&client, &mut info).await;
            update_info(status, &reader_ip, info.clone()).await;
            let response = reader_control_response(
                request_id,
                reader_ip,
                true,
                None,
                Some(to_protocol_reader_info(&info)),
            );
            session.send_message(&response).await
        }

        ReaderControlAction::SetReadMode { mode, timeout } => {
            let read_mode = protocol_to_ipico_read_mode(mode);
            match client.set_config3(read_mode, timeout).await {
                Ok(()) => {
                    let mut info = get_cached_info(status, &reader_ip).await;
                    info.config = Some(forwarder::reader_control::Config3Info {
                        mode: read_mode,
                        timeout,
                    });
                    info.clock = None;
                    forwarder::reader_control::run_status_poll_merge_successes(&client, &mut info)
                        .await;
                    update_info(status, &reader_ip, info.clone()).await;
                    let response = reader_control_response(
                        request_id,
                        reader_ip,
                        true,
                        None,
                        Some(to_protocol_reader_info(&info)),
                    );
                    session.send_message(&response).await
                }
                Err(e) => {
                    let response = reader_control_response(
                        request_id,
                        reader_ip,
                        false,
                        Some(e.to_string()),
                        None,
                    );
                    session.send_message(&response).await
                }
            }
        }

        ReaderControlAction::SetTto { enabled } => {
            let current = match client.get_tag_message_format().await {
                Ok(format) => format,
                Err(e) => {
                    let response = reader_control_response(
                        request_id,
                        reader_ip,
                        false,
                        Some(e.to_string()),
                        None,
                    );
                    return session.send_message(&response).await;
                }
            };
            let updated = current.with_tto_enabled(enabled);
            if let Err(e) = client.set_tag_message_format(updated).await {
                let response = reader_control_response(
                    request_id,
                    reader_ip,
                    false,
                    Some(e.to_string()),
                    None,
                );
                return session.send_message(&response).await;
            }
            // Verify and update info
            match client.get_tag_message_format().await {
                Ok(format) => {
                    let mut info = get_cached_info(status, &reader_ip).await;
                    info.tto_enabled = Some(format.tto_enabled());
                    forwarder::reader_control::run_status_poll_merge_successes(&client, &mut info)
                        .await;
                    update_info(status, &reader_ip, info.clone()).await;
                    let response = reader_control_response(
                        request_id,
                        reader_ip,
                        true,
                        None,
                        Some(to_protocol_reader_info(&info)),
                    );
                    session.send_message(&response).await
                }
                Err(e) => {
                    let response = reader_control_response(
                        request_id,
                        reader_ip,
                        false,
                        Some(format!("set ok but verify failed: {}", e)),
                        None,
                    );
                    session.send_message(&response).await
                }
            }
        }

        ReaderControlAction::SetRecording { enabled } => {
            match client.set_recording(enabled).await {
                Ok(ext) => {
                    let mut info = get_cached_info(status, &reader_ip).await;
                    info.recording = Some(ext.recording_state.is_recording());
                    info.estimated_stored_reads = Some(ext.estimated_stored_reads());
                    forwarder::reader_control::run_status_poll_merge_successes(&client, &mut info)
                        .await;
                    update_info(status, &reader_ip, info.clone()).await;
                    let response = reader_control_response(
                        request_id,
                        reader_ip,
                        true,
                        None,
                        Some(to_protocol_reader_info(&info)),
                    );
                    session.send_message(&response).await
                }
                Err(e) => {
                    let response = reader_control_response(
                        request_id,
                        reader_ip,
                        false,
                        Some(e.to_string()),
                        None,
                    );
                    session.send_message(&response).await
                }
            }
        }

        ReaderControlAction::ClearRecords => {
            // Spawn background task — clear_records takes ~10s
            let bg_client = client.clone();
            let bg_status = status.clone();
            let bg_reader_ip = reader_ip.clone();
            tokio::spawn(async move {
                match bg_client.clear_records().await {
                    Ok(()) => {
                        // Refresh info after clear
                        let mut info = bg_status
                            .get_reader_info(&bg_reader_ip)
                            .await
                            .unwrap_or_default();
                        forwarder::reader_control::run_status_poll(&bg_client, &mut info).await;
                        bg_status
                            .update_reader_info_unless_disconnected(&bg_reader_ip, info)
                            .await;
                    }
                    Err(e) => {
                        warn!(reader_ip = %bg_reader_ip, error = %e, "clear_records failed");
                    }
                }
            });
            // Return success immediately
            let response = reader_control_response(request_id, reader_ip, true, None, None);
            session.send_message(&response).await
        }

        ReaderControlAction::StartDownload => {
            let tracker = {
                status
                    .download_trackers()
                    .read()
                    .unwrap_or_else(|e| {
                        warn!("recovered from poisoned RwLock");
                        e.into_inner()
                    })
                    .get(&reader_ip)
                    .cloned()
            };
            let Some(tracker) = tracker else {
                let response = reader_control_response(
                    request_id,
                    reader_ip,
                    false,
                    Some("reader not connected".to_owned()),
                    None,
                );
                return session.send_message(&response).await;
            };

            // Check current state and prepare
            {
                let mut dt = tracker.lock().await;
                match dt.state() {
                    forwarder::reader_control::DownloadState::Starting
                    | forwarder::reader_control::DownloadState::Downloading => {
                        let response = reader_control_response(
                            request_id,
                            reader_ip,
                            false,
                            Some("download already in progress".to_owned()),
                            None,
                        );
                        return session.send_message(&response).await;
                    }
                    forwarder::reader_control::DownloadState::Complete
                    | forwarder::reader_control::DownloadState::Error(_) => {
                        dt.reset();
                    }
                    forwarder::reader_control::DownloadState::Idle => {}
                }
                dt.begin_startup();
            }

            // Spawn background download task
            let bg_client = client.clone();
            let bg_tracker = tracker.clone();
            let bg_reader_ip = reader_ip.clone();
            tokio::spawn(async move {
                match bg_client.start_download().await {
                    Ok(ext) => {
                        let mut dt = bg_tracker.lock().await;
                        dt.start(ext.stored_data_extent);
                    }
                    Err(e) => {
                        warn!(reader_ip = %bg_reader_ip, error = %e, "download start failed");
                        let mut dt = bg_tracker.lock().await;
                        dt.fail(format!("{}", e));
                    }
                }
            });

            let response = reader_control_response(request_id, reader_ip, true, None, None);
            session.send_message(&response).await
        }

        ReaderControlAction::StopDownload => match client.stop_download().await {
            Ok(()) => {
                let response = reader_control_response(request_id, reader_ip, true, None, None);
                session.send_message(&response).await
            }
            Err(e) => {
                let response = reader_control_response(
                    request_id,
                    reader_ip,
                    false,
                    Some(e.to_string()),
                    None,
                );
                session.send_message(&response).await
            }
        },

        ReaderControlAction::Reconnect => {
            let notify = {
                status
                    .reconnect_notifies()
                    .read()
                    .unwrap_or_else(|e| {
                        warn!("recovered from poisoned RwLock");
                        e.into_inner()
                    })
                    .get(&reader_ip)
                    .cloned()
            };
            match notify {
                Some(n) => {
                    n.notify_one();
                    let response = reader_control_response(request_id, reader_ip, true, None, None);
                    session.send_message(&response).await
                }
                None => {
                    let response = reader_control_response(
                        request_id,
                        reader_ip,
                        false,
                        Some("reader not found".to_owned()),
                        None,
                    );
                    session.send_message(&response).await
                }
            }
        }

        ReaderControlAction::SyncClock => {
            // Spawn background task — clock sync takes 3-5+ seconds (RTT probes,
            // pre-set wait, set_date_time, verify wait, verification read).
            let bg_client = client.clone();
            let bg_status = status.clone();
            let bg_reader_ip = reader_ip.clone();
            tokio::spawn(async move {
                const SYNC_DELAY_MS: u64 = 500;

                // Step 1: estimate one-way latency via RTT probes
                let mut rtts = Vec::with_capacity(3);
                for i in 0..3usize {
                    let start = std::time::Instant::now();
                    match bg_client.get_date_time().await {
                        Ok(_) => rtts.push(start.elapsed()),
                        Err(e) => warn!(probe = i + 1, error = %e, "RTT probe failed"),
                    }
                }
                if rtts.is_empty() {
                    warn!(reader_ip = %bg_reader_ip, "all RTT probes failed; cannot estimate latency for clock sync");
                    let mut info = bg_status
                        .get_reader_info(&bg_reader_ip)
                        .await
                        .unwrap_or_default();
                    info.clock = None;
                    bg_status
                        .update_reader_info_unless_disconnected(&bg_reader_ip, info)
                        .await;
                    return;
                }
                rtts.sort();
                let one_way = rtts[rtts.len() / 2] / 2;

                // Step 2: compute sync timing
                use chrono::{Datelike, Timelike};
                let wall_now = chrono::Local::now();
                let (target_boundary, pre_set_wait) =
                    forwarder::status_http::compute_sync_timing(wall_now, one_way, SYNC_DELAY_MS);
                if !pre_set_wait.is_zero() {
                    sleep(pre_set_wait).await;
                }

                let year = (target_boundary.year() % 100) as u8;
                let month = target_boundary.month() as u8;
                let day = target_boundary.day() as u8;
                let dow = target_boundary.weekday().num_days_from_sunday() as u8;
                let hour = target_boundary.hour() as u8;
                let minute = target_boundary.minute() as u8;
                let second = target_boundary.second() as u8;

                if let Err(e) = bg_client
                    .set_date_time(year, month, day, dow, hour, minute, second)
                    .await
                {
                    // Clear stale clock info
                    let mut info = bg_status
                        .get_reader_info(&bg_reader_ip)
                        .await
                        .unwrap_or_default();
                    info.clock = None;
                    bg_status
                        .update_reader_info_unless_disconnected(&bg_reader_ip, info)
                        .await;
                    warn!(reader_ip = %bg_reader_ip, error = %e, "set_date_time failed during clock sync");
                    return;
                }

                // Step 3: wait for sync to complete, then verify
                let verify_wait = std::time::Duration::from_millis(SYNC_DELAY_MS) + one_way;
                sleep(verify_wait).await;

                match bg_client.get_date_time().await {
                    Ok(dt) => {
                        let reader_iso = dt.to_iso_string();
                        let verify_now = chrono::Local::now();
                        let drift_ms = chrono::NaiveDateTime::parse_from_str(
                            &reader_iso,
                            "%Y-%m-%dT%H:%M:%S%.3f",
                        )
                        .ok()
                        .map(|reader_naive| {
                            verify_now
                                .naive_local()
                                .signed_duration_since(reader_naive)
                                .num_milliseconds()
                        });

                        let mut info = bg_status
                            .get_reader_info(&bg_reader_ip)
                            .await
                            .unwrap_or_default();
                        info.clock = drift_ms.map(|d| forwarder::reader_control::ClockInfo {
                            reader_clock: reader_iso,
                            drift_ms: d,
                        });
                        bg_status
                            .update_reader_info_unless_disconnected(&bg_reader_ip, info)
                            .await;
                    }
                    Err(e) => {
                        warn!(reader_ip = %bg_reader_ip, error = %e, "set_date_time ok but verify failed during clock sync");
                    }
                }
            });
            // Return success immediately
            let response = reader_control_response(request_id, reader_ip, true, None, None);
            session.send_message(&response).await
        }
    }
}

// ---------------------------------------------------------------------------
// Uplink task: WebSocket connect → replay → send batches → receive acks
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_uplink(
    cfg: ForwarderConfig,
    forwarder_id: String,
    reader_ips: Vec<String>,
    journal: Arc<Mutex<Journal>>,
    mut shutdown_rx: watch::Receiver<bool>,
    status: StatusServer,
    config_state: Arc<ConfigState>,
    subsystem: Arc<Mutex<SubsystemStatus>>,
    restart_signal: Arc<Notify>,
    logger: Arc<rt_ui_log::UiLogger<ForwarderUiEvent>>,
    mut reader_status_rx: tokio::sync::mpsc::UnboundedReceiver<rt_protocol::ReaderStatusUpdate>,
) {
    let ui_tx = status.ui_sender();
    let server_url = format!(
        "{}{}",
        cfg.server.base_url.trim_end_matches('/'),
        cfg.server.forwarders_ws_path
    );
    // Convert http(s) scheme to ws(s) scheme
    let ws_url = if server_url.starts_with("https://") {
        server_url.replacen("https://", "wss://", 1)
    } else if server_url.starts_with("http://") {
        server_url.replacen("http://", "ws://", 1)
    } else {
        server_url
    };

    let uplink_cfg = UplinkConfig {
        server_url: ws_url.clone(),
        token: cfg.token.clone(),
        forwarder_id: forwarder_id.clone(),
        display_name: cfg.display_name.clone(),
        batch_flush_ms: cfg.uplink.batch_flush_ms,
        batch_max_events: cfg.uplink.batch_max_events,
        ack_timeout_secs: cfg.uplink.ack_timeout_secs,
    };

    let mut backoff_secs: u64 = 1;

    loop {
        if *shutdown_rx.borrow() {
            info!("uplink task stopping (shutdown)");
            return;
        }

        logger.log(format!("uplink connecting to {}", ws_url));

        let mut session =
            match UplinkSession::connect_with_readers(uplink_cfg.clone(), reader_ips.clone()).await
            {
                Ok(s) => {
                    logger.log(format!("uplink connected (session {})", s.session_id()));
                    status.set_uplink_connected(true).await;
                    backoff_secs = 1;
                    s
                }
                Err(e) => {
                    logger.log_at(
                        UiLogLevel::Warn,
                        format!(
                            "uplink connect failed: {}; retrying in {}s",
                            e, backoff_secs
                        ),
                    );
                    let delay = Duration::from_secs(backoff_secs);
                    tokio::select! {
                        _ = sleep(delay) => {}
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                return;
                            }
                        }
                    }
                    backoff_secs = (backoff_secs * 2).min(30);
                    continue;
                }
            };

        // Send initial reader status burst
        let mut burst_ok = true;
        {
            let ss = subsystem.lock().await;
            for (ip, reader) in ss.readers() {
                let connected = reader.state == ReaderConnectionState::Connected;
                let update = WsMessage::ReaderStatusUpdate(rt_protocol::ReaderStatusUpdate {
                    reader_ip: ip.clone(),
                    connected,
                });
                if session.send_message(&update).await.is_err() {
                    warn!(reader_ip = %ip, "failed to send reader status during initial burst; reconnecting");
                    burst_ok = false;
                    break;
                }
            }
        }

        if !burst_ok {
            status.set_uplink_connected(false).await;
            continue; // reconnect via outer loop
        }

        // Drain any reader status updates that arrived during connect/burst
        let mut drain_ok = true;
        while let Ok(update) = reader_status_rx.try_recv() {
            let msg = WsMessage::ReaderStatusUpdate(update);
            if session.send_message(&msg).await.is_err() {
                warn!("failed to send reader status during drain; reconnecting");
                drain_ok = false;
                break;
            }
        }

        if !drain_ok {
            status.set_uplink_connected(false).await;
            continue; // reconnect via outer loop
        }

        // Replay unacked events from journal
        let replay_results = {
            let j = journal.lock().await;
            let engine = ReplayEngine::new();
            let mut all: Vec<(String, i64, Vec<forwarder::storage::journal::JournalEvent>)> =
                Vec::new();
            for ip in &reader_ips {
                match engine.pending_events(&j, ip) {
                    Ok(results) => {
                        for rr in results {
                            all.push((rr.stream_key, rr.stream_epoch, rr.events));
                        }
                    }
                    Err(e) => {
                        warn!(reader_ip = %ip, error = %e, "replay error; events skipped until next reconnect");
                        logger.log_at(
                            UiLogLevel::Error,
                            format!("replay error for reader {}: {}; events skipped until next reconnect", ip, e),
                        );
                    }
                }
            }
            all
        };

        // Send replayed events
        let mut reconnect_after_replay = false;
        'replay: for (stream_key, _stream_epoch, events) in replay_results {
            if events.is_empty() {
                continue;
            }
            let read_events: Vec<ReadEvent> = events
                .iter()
                .map(|ev| ReadEvent {
                    forwarder_id: forwarder_id.clone(),
                    reader_ip: ev.stream_key.clone(),
                    stream_epoch: ev.stream_epoch,
                    seq: ev.seq,
                    reader_timestamp: ev.reader_timestamp.clone().unwrap_or_default(),
                    raw_frame: ev.raw_frame.clone(),
                    read_type: ev.read_type.clone(),
                })
                .collect();

            logger.log_at(
                UiLogLevel::Debug,
                format!(
                    "replaying {} unacked events for {}",
                    read_events.len(),
                    stream_key
                ),
            );

            for replay_chunk in chunk_for_replay(read_events, cfg.uplink.batch_max_events) {
                let send_result = session.send_batch(replay_chunk).await;
                reconnect_after_replay = should_reconnect_after_replay_send(&send_result);

                match send_result {
                    Ok(SendBatchResult::Ack(ack)) => {
                        process_ack(&journal, &ack).await;
                    }
                    Ok(SendBatchResult::EpochReset(cmd)) => {
                        logger.log(format!(
                            "epoch reset for {}; bumping journal and reconnecting",
                            cmd.reader_ip
                        ));
                        let mut j = journal.lock().await;
                        if let Err(e) = j.bump_epoch(&cmd.reader_ip, cmd.new_stream_epoch) {
                            logger.log_at(
                                UiLogLevel::Warn,
                                format!("failed to bump epoch in journal: {}", e),
                            );
                        }
                        break 'replay;
                    }
                    Ok(SendBatchResult::ConfigGet(req)) => {
                        let msg = WsMessage::ConfigGetRequest(req);
                        if let Err(e) = handle_config_message(
                            &mut session,
                            msg,
                            &config_state,
                            &subsystem,
                            &ui_tx,
                        )
                        .await
                        {
                            warn!(error = %e, "config get handler failed during replay");
                            reconnect_after_replay = true;
                            break 'replay;
                        }
                    }
                    Ok(SendBatchResult::ConfigSet(req)) => {
                        let msg = WsMessage::ConfigSetRequest(req);
                        if let Err(e) = handle_config_message(
                            &mut session,
                            msg,
                            &config_state,
                            &subsystem,
                            &ui_tx,
                        )
                        .await
                        {
                            warn!(error = %e, "config set handler failed during replay");
                            reconnect_after_replay = true;
                            break 'replay;
                        }
                    }
                    Ok(SendBatchResult::Restart(req)) => {
                        if let Err(e) =
                            handle_restart_message(&mut session, req, &restart_signal).await
                        {
                            warn!(error = %e, "restart handler failed during replay");
                            reconnect_after_replay = true;
                            break 'replay;
                        }
                    }
                    Ok(SendBatchResult::ReaderControl(req)) => {
                        let reader_ip = req.reader_ip.clone();
                        let action = format!("{:?}", req.action);
                        if let Err(e) =
                            handle_reader_control_message(&mut session, req, &status).await
                        {
                            warn!(reader_ip = %reader_ip, action = %action, error = %e, "reader control handler failed during replay");
                            reconnect_after_replay = true;
                            break 'replay;
                        }
                    }
                    Err(e) => {
                        logger.log_at(
                            UiLogLevel::Warn,
                            format!("replay send failed: {}; reconnecting", e),
                        );
                        reconnect_after_replay = true;
                    }
                }

                if reconnect_after_replay {
                    break 'replay;
                }
            }
        }

        if reconnect_after_replay {
            continue;
        }

        // Main uplink loop: periodically send new events and wait for acks
        let flush_interval = Duration::from_millis(cfg.uplink.batch_flush_ms);
        let mut ui_rx = ui_tx.subscribe();

        'uplink: loop {
            if *shutdown_rx.borrow() {
                info!("uplink task stopping (shutdown)");
                return;
            }

            // Drain UI events and forward reader state changes upstream
            loop {
                match ui_rx.try_recv() {
                    Ok(ForwarderUiEvent::ReaderUpdated { ip, state, .. }) => {
                        let proto_state = ui_to_protocol_connection_state(state);
                        let msg = WsMessage::ReaderInfoUpdate(rt_protocol::ReaderInfoUpdate {
                            reader_ip: ip.clone(),
                            state: proto_state,
                            reader_info: None,
                        });
                        if let Err(e) = session.send_message(&msg).await {
                            warn!(error = %e, reader_ip = %ip, "failed to send ReaderInfoUpdate upstream");
                            break 'uplink;
                        }
                    }
                    Ok(ForwarderUiEvent::ReaderInfoUpdated { ip, info }) => {
                        let proto_state = {
                            let ss = subsystem.lock().await;
                            ss.reader_connection_state(&ip)
                                .map(status_to_protocol_connection_state)
                                .unwrap_or(rt_protocol::ReaderConnectionState::Disconnected)
                        };
                        let msg = WsMessage::ReaderInfoUpdate(rt_protocol::ReaderInfoUpdate {
                            reader_ip: ip.clone(),
                            state: proto_state,
                            reader_info: Some(to_protocol_reader_info(&info)),
                        });
                        if let Err(e) = session.send_message(&msg).await {
                            warn!(error = %e, reader_ip = %ip, "failed to send ReaderInfoUpdate upstream");
                            break 'uplink;
                        }
                    }
                    Ok(_) => {
                        // Ignore other UI events (StatusChanged, LogEntry, etc.)
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Empty) => break,
                    Err(tokio::sync::broadcast::error::TryRecvError::Lagged(n)) => {
                        warn!(
                            skipped = n,
                            "ui_rx lagged; some reader updates were not forwarded"
                        );
                    }
                    Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break 'uplink,
                }
            }

            // Wait for flush interval, shutdown, reader status updates, or incoming config messages
            tokio::select! {
                _ = sleep(flush_interval) => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        return;
                    }
                }
                Some(update) = reader_status_rx.recv() => {
                    let msg = WsMessage::ReaderStatusUpdate(update);
                    if session.send_message(&msg).await.is_err() {
                        warn!("failed to send reader status update; reconnecting");
                        break 'uplink;
                    }
                    continue 'uplink;
                }
                result = session.recv_message() => {
                    match result {
                        Ok(msg @ WsMessage::ConfigGetRequest(_)) | Ok(msg @ WsMessage::ConfigSetRequest(_)) => {
                            match &msg {
                                WsMessage::ConfigGetRequest(_) => logger.log("server requested config"),
                                WsMessage::ConfigSetRequest(req) => logger.log(format!("server updated config section '{}'", req.section)),
                                _ => {}
                            }
                            if let Err(e) = handle_config_message(&mut session, msg, &config_state, &subsystem, &ui_tx).await {
                                warn!(error = %e, "config handler failed during idle");
                                break 'uplink;
                            }
                            continue 'uplink;
                        }
                        Ok(WsMessage::RestartRequest(req)) => {
                            logger.log("restart requested by server");
                            if let Err(e) = handle_restart_message(&mut session, req, &restart_signal).await {
                                warn!(error = %e, "restart handler failed during idle");
                                break 'uplink;
                            }
                            continue 'uplink;
                        }
                        Ok(WsMessage::ReaderControlRequest(req)) => {
                            logger.log(format!("reader control request: {:?} for {}", req.action, req.reader_ip));
                            let reader_ip = req.reader_ip.clone();
                            let action = format!("{:?}", req.action);
                            if let Err(e) = handle_reader_control_message(
                                &mut session,
                                req,
                                &status,
                            ).await {
                                warn!(reader_ip = %reader_ip, action = %action, error = %e, "reader control handler failed during idle");
                                break 'uplink;
                            }
                            continue 'uplink;
                        }
                        Ok(WsMessage::Heartbeat(_)) => { continue 'uplink; }
                        Ok(WsMessage::EpochResetCommand(cmd)) => {
                            info!(reader_ip = %cmd.reader_ip, new_epoch = cmd.new_stream_epoch, "epoch reset during idle");
                            let mut j = journal.lock().await;
                            if let Err(e) = j.bump_epoch(&cmd.reader_ip, cmd.new_stream_epoch) {
                                warn!(error = %e, "failed to bump epoch");
                            }
                            break 'uplink;
                        }
                        Ok(WsMessage::Error(e)) => {
                            warn!(code = %e.code, msg = %e.message, "server error during idle");
                            break 'uplink;
                        }
                        Err(e) => {
                            warn!(error = %e, "ws receive failed during idle");
                            break 'uplink;
                        }
                        Ok(other) => {
                            tracing::debug!(msg_type = ?std::mem::discriminant(&other), "ignoring unexpected message during idle");
                            continue 'uplink;
                        }
                    }
                }
            }

            // Gather new events since last ack cursor
            let pending: Vec<ReadEvent> = {
                let j = journal.lock().await;
                let engine = ReplayEngine::new();
                let mut batch = Vec::new();
                for ip in &reader_ips {
                    match engine.pending_events(&j, ip) {
                        Ok(results) => {
                            for rr in results {
                                for ev in rr.events {
                                    batch.push(ReadEvent {
                                        forwarder_id: forwarder_id.clone(),
                                        reader_ip: ev.stream_key.clone(),
                                        stream_epoch: ev.stream_epoch,
                                        seq: ev.seq,
                                        reader_timestamp: ev
                                            .reader_timestamp
                                            .clone()
                                            .unwrap_or_default(),
                                        raw_frame: ev.raw_frame.clone(),
                                        read_type: ev.read_type.clone(),
                                    });
                                    if batch.len() >= cfg.uplink.batch_max_events as usize {
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!(reader_ip = %ip, error = %e, "pending_events error in uplink loop");
                        }
                    }
                }
                batch
            };

            if pending.is_empty() {
                continue;
            }

            logger.log_at(
                UiLogLevel::Debug,
                format!("sent batch of {} events", pending.len()),
            );

            match session.send_batch(pending).await {
                Ok(SendBatchResult::Ack(ack)) => {
                    process_ack(&journal, &ack).await;
                }
                Ok(SendBatchResult::EpochReset(cmd)) => {
                    logger.log(format!(
                        "epoch reset for {}; bumping journal",
                        cmd.reader_ip
                    ));
                    let mut j = journal.lock().await;
                    if let Err(e) = j.bump_epoch(&cmd.reader_ip, cmd.new_stream_epoch) {
                        logger.log_at(
                            UiLogLevel::Warn,
                            format!("failed to bump epoch in journal: {}", e),
                        );
                    }
                    break 'uplink;
                }
                Ok(SendBatchResult::ConfigGet(req)) => {
                    logger.log("server requested config");
                    let msg = WsMessage::ConfigGetRequest(req);
                    if let Err(e) =
                        handle_config_message(&mut session, msg, &config_state, &subsystem, &ui_tx)
                            .await
                    {
                        warn!(error = %e, "config get handler failed");
                        break 'uplink;
                    }
                }
                Ok(SendBatchResult::ConfigSet(req)) => {
                    logger.log(format!("server updated config section '{}'", req.section));
                    let msg = WsMessage::ConfigSetRequest(req);
                    if let Err(e) =
                        handle_config_message(&mut session, msg, &config_state, &subsystem, &ui_tx)
                            .await
                    {
                        warn!(error = %e, "config set handler failed");
                        break 'uplink;
                    }
                }
                Ok(SendBatchResult::Restart(req)) => {
                    if let Err(e) = handle_restart_message(&mut session, req, &restart_signal).await
                    {
                        warn!(error = %e, "restart handler failed");
                        break 'uplink;
                    }
                }
                Ok(SendBatchResult::ReaderControl(req)) => {
                    let reader_ip = req.reader_ip.clone();
                    let action = format!("{:?}", req.action);
                    if let Err(e) = handle_reader_control_message(&mut session, req, &status).await
                    {
                        warn!(reader_ip = %reader_ip, action = %action, error = %e, "reader control handler failed");
                        break 'uplink;
                    }
                }
                Err(e) => {
                    warn!(error = %e, "send_batch failed, reconnecting uplink");
                    break 'uplink;
                }
            }
        }

        // Reconnect with backoff
        status.set_uplink_connected(false).await;
        let delay = Duration::from_secs(backoff_secs);
        logger.log_at(
            UiLogLevel::Warn,
            format!("uplink disconnected; reconnecting in {}s", backoff_secs),
        );
        tokio::select! {
            _ = sleep(delay) => {}
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    return;
                }
            }
        }
        backoff_secs = (backoff_secs * 2).min(30);
    }
}
