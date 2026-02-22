// forwarder: Reads from IPICO timing hardware and forwards events to the server.
//
// Runtime event loop: wires together journal, local fanout, IPICO TCP readers,
// WebSocket uplink/replay, and the status HTTP server.

use forwarder::config::ForwarderConfig;
use forwarder::discovery::expand_target;
use forwarder::local_fanout::FanoutServer;
use forwarder::replay::ReplayEngine;
use forwarder::status_http::{
    ConfigState, ReaderConnectionState, StatusConfig, StatusServer, SubsystemStatus,
};
use forwarder::storage::journal::Journal;
use forwarder::ui_events::ForwarderUiEvent;
use forwarder::uplink::{SendBatchResult, UplinkConfig, UplinkError, UplinkSession};
use forwarder::uplink_replay::should_reconnect_after_replay_send;
use ipico_core::read::ChipRead;
use rt_protocol::{ReadEvent, WsMessage};
use rt_ui_log::UiLogLevel;
use sha2::{Digest, Sha256};
use std::convert::TryFrom;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{watch, Mutex, Notify};
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Derive the forwarder_id from the raw token bytes.
///
/// SHA-256 hex of token bytes, first 16 hex chars, prefixed with "fwd-".
fn derive_forwarder_id(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    let result = hasher.finalize();
    let hex = format!("{:x}", result);
    format!("fwd-{}", &hex[..16])
}

/// Detect the local IP used to reach a given target IP.
///
/// Uses a UDP socket connect (no traffic sent) to let the OS choose the
/// outgoing interface, then reads back the local address.
fn detect_local_ip(target_ip: &str) -> Option<String> {
    let dest = format!("{}:10000", target_ip);
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(&dest).ok()?;
    let local_addr = socket.local_addr().ok()?;
    Some(local_addr.ip().to_string())
}

async fn mark_reader_disconnected(status: &StatusServer, reader_ip: &str) {
    status
        .update_reader_state(reader_ip, ReaderConnectionState::Disconnected)
        .await;
}

#[derive(Debug)]
enum JournalAppendError {
    StreamState(String),
    NextSeq(String),
    Insert(String),
}

fn append_read_to_journal(
    journal: &mut Journal,
    stream_key: &str,
    reader_timestamp: Option<&str>,
    raw_line: &str,
    read_type: &str,
) -> Result<(i64, i64), JournalAppendError> {
    let (epoch, _) = journal
        .current_epoch_and_next_seq(stream_key)
        .map_err(|e| JournalAppendError::StreamState(e.to_string()))?;
    let seq = journal
        .next_seq(stream_key)
        .map_err(|e| JournalAppendError::NextSeq(e.to_string()))?;
    journal
        .insert_event(
            stream_key,
            epoch,
            seq,
            reader_timestamp,
            raw_line,
            read_type,
        )
        .map_err(|e| JournalAppendError::Insert(e.to_string()))?;
    Ok((epoch, seq))
}

fn chunk_for_replay(events: Vec<ReadEvent>, max_events_per_batch: u32) -> Vec<Vec<ReadEvent>> {
    let chunk_size = max_events_per_batch.max(1) as usize;
    events
        .chunks(chunk_size)
        .map(std::borrow::ToOwned::to_owned)
        .collect()
}

// ---------------------------------------------------------------------------
// Reader task: TCP connect → parse IPICO frames → journal + fanout
// ---------------------------------------------------------------------------

async fn run_reader(
    reader_ip: String,
    reader_port: u16,
    fanout_addr: SocketAddr,
    journal: Arc<Mutex<Journal>>,
    mut shutdown_rx: watch::Receiver<bool>,
    status: StatusServer,
    logger: Arc<rt_ui_log::UiLogger<ForwarderUiEvent>>,
) {
    let target_addr = format!("{}:{}", reader_ip, reader_port);
    let stream_key = format!("{}:{}", reader_ip, reader_port);
    let mut backoff_secs: u64 = 1;

    loop {
        // Check for shutdown before attempting connect
        if *shutdown_rx.borrow() {
            info!(reader_ip = %reader_ip, "reader task stopping (shutdown)");
            return;
        }

        info!(reader_ip = %reader_ip, target = %target_addr, "connecting to reader");

        status
            .update_reader_state(&stream_key, ReaderConnectionState::Connecting)
            .await;

        let stream = match TcpStream::connect(&target_addr).await {
            Ok(s) => {
                logger.log(format!("reader {} connected", reader_ip));
                backoff_secs = 1; // reset backoff on successful connect
                status
                    .update_reader_state(&stream_key, ReaderConnectionState::Connected)
                    .await;
                s
            }
            Err(e) => {
                logger.log_at(
                    UiLogLevel::Warn,
                    format!(
                        "reader {} connect failed: {}; retrying in {}s",
                        reader_ip, e, backoff_secs
                    ),
                );
                mark_reader_disconnected(&status, &stream_key).await;
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

        // Initialize stream state in journal on first connect
        {
            let mut j = journal.lock().await;
            let epoch = 1_i64;
            if let Err(e) = j.ensure_stream_state(&stream_key, epoch) {
                logger.log_at(
                    UiLogLevel::Warn,
                    format!("reader {} journal init failed: {}", reader_ip, e),
                );
            }
        }

        let mut reader = BufReader::new(stream);
        let mut line_buf = String::new();

        loop {
            line_buf.clear();

            // Wait for a line or shutdown
            let read_result = tokio::select! {
                result = reader.read_line(&mut line_buf) => result,
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!(reader_ip = %reader_ip, "reader task stopping (shutdown)");
                        return;
                    }
                    continue;
                }
            };

            match read_result {
                Err(e) => {
                    logger.log_at(
                        UiLogLevel::Warn,
                        format!("reader {} read error: {}; reconnecting", reader_ip, e),
                    );
                    mark_reader_disconnected(&status, &stream_key).await;
                    break;
                }
                Ok(0) => {
                    logger.log_at(
                        UiLogLevel::Warn,
                        format!("reader {} connection closed; reconnecting", reader_ip),
                    );
                    mark_reader_disconnected(&status, &stream_key).await;
                    break;
                }
                Ok(_) => {}
            }

            let raw_line = line_buf.trim_end_matches(['\r', '\n']).to_owned();
            if raw_line.is_empty() {
                continue;
            }

            // Parse IPICO chip read to validate and extract metadata
            let parsed_read_type;
            let reader_timestamp;
            match ChipRead::try_from(raw_line.as_str()) {
                Ok(chip) => {
                    reader_timestamp = Some(chip.timestamp.to_string());
                    parsed_read_type = chip.read_type.as_str().to_owned();
                }
                Err(_) => {
                    // Line is not a valid IPICO read — log and skip
                    logger.log_at(
                        UiLogLevel::Warn,
                        format!("reader {} skipped unparseable line", reader_ip),
                    );
                    continue;
                }
            }

            // Write to journal. Keep status updates out of the DB lock scope.
            let append_result = {
                let mut j = journal.lock().await;
                append_read_to_journal(
                    &mut j,
                    &stream_key,
                    reader_timestamp.as_deref(),
                    &raw_line,
                    &parsed_read_type,
                )
            };
            let (epoch, seq) = match append_result {
                Ok(v) => v,
                Err(JournalAppendError::StreamState(e)) => {
                    logger.log_at(
                        UiLogLevel::Error,
                        format!("reader {} journal error (epoch): {}", reader_ip, e),
                    );
                    mark_reader_disconnected(&status, &stream_key).await;
                    break;
                }
                Err(JournalAppendError::NextSeq(e)) => {
                    logger.log_at(
                        UiLogLevel::Error,
                        format!("reader {} journal error (seq): {}", reader_ip, e),
                    );
                    mark_reader_disconnected(&status, &stream_key).await;
                    break;
                }
                Err(JournalAppendError::Insert(e)) => {
                    logger.log_at(
                        UiLogLevel::Error,
                        format!("reader {} journal insert failed: {}", reader_ip, e),
                    );
                    mark_reader_disconnected(&status, &stream_key).await;
                    break;
                }
            };

            debug!(
                reader_ip = %reader_ip,
                epoch = epoch,
                seq = seq,
                "event journaled"
            );

            // Fan out raw bytes to local TCP consumers
            let raw_bytes = format!("{}\n", raw_line).into_bytes();
            if let Err(e) = FanoutServer::push_to_addr(fanout_addr, raw_bytes).await {
                warn!(reader_ip = %reader_ip, error = %e, "local fanout push failed");
                // Non-fatal: local fanout failure doesn't break uplink path
            }

            status.record_read(&stream_key).await;
        }

        // Reconnect with backoff
        let delay = Duration::from_secs(backoff_secs);
        info!(
            reader_ip = %reader_ip,
            backoff_secs = backoff_secs,
            "waiting before reconnect"
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

// ---------------------------------------------------------------------------
// Config message handler (used by uplink loop)
// ---------------------------------------------------------------------------

async fn handle_config_message(
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
                        let err_msg = serde_json::from_str::<serde_json::Value>(&err_json)
                            .ok()
                            .and_then(|v| {
                                v.get("error")
                                    .and_then(|e| e.as_str())
                                    .map(|s| s.to_owned())
                            })
                            .unwrap_or(err_json);
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
                        let err_msg = serde_json::from_str::<serde_json::Value>(&err_json)
                            .ok()
                            .and_then(|v| {
                                v.get("error")
                                    .and_then(|e| e.as_str())
                                    .map(|s| s.to_owned())
                            })
                            .unwrap_or(err_json);
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
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Restart message handler (used by uplink loop)
// ---------------------------------------------------------------------------

async fn handle_restart_message(
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
// Uplink task: WebSocket connect → replay → send batches → receive acks
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
async fn run_uplink(
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
        batch_mode: cfg.uplink.batch_mode.clone(),
        batch_flush_ms: cfg.uplink.batch_flush_ms,
        batch_max_events: cfg.uplink.batch_max_events,
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
                        logger.log_at(
                            UiLogLevel::Warn,
                            format!("replay error for reader {}: {}", ip, e),
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
                    stream_epoch: ev.stream_epoch as u64,
                    seq: ev.seq as u64,
                    reader_timestamp: ev.reader_timestamp.clone().unwrap_or_default(),
                    raw_read_line: ev.raw_read_line.clone(),
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
                        let mut j = journal.lock().await;
                        for entry in &ack.entries {
                            if let Err(e) = j.update_ack_cursor(
                                &entry.reader_ip,
                                entry.stream_epoch as i64,
                                entry.last_seq as i64,
                            ) {
                                warn!(error = %e, "failed to update ack cursor");
                            }
                        }
                    }
                    Ok(SendBatchResult::EpochReset(cmd)) => {
                        logger.log(format!(
                            "epoch reset for {}; bumping journal and reconnecting",
                            cmd.reader_ip
                        ));
                        let mut j = journal.lock().await;
                        if let Err(e) = j.bump_epoch(&cmd.reader_ip, cmd.new_stream_epoch as i64) {
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
                    Err(e) => {
                        logger.log_at(
                            UiLogLevel::Warn,
                            format!("replay send failed: {}; reconnecting", e),
                        );
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

        'uplink: loop {
            if *shutdown_rx.borrow() {
                info!("uplink task stopping (shutdown)");
                return;
            }

            // Wait for flush interval, shutdown, or incoming config messages
            tokio::select! {
                _ = sleep(flush_interval) => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        return;
                    }
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
                        Ok(WsMessage::Heartbeat(_)) => { continue 'uplink; }
                        Ok(WsMessage::EpochResetCommand(cmd)) => {
                            info!(reader_ip = %cmd.reader_ip, new_epoch = cmd.new_stream_epoch, "epoch reset during idle");
                            let mut j = journal.lock().await;
                            if let Err(e) = j.bump_epoch(&cmd.reader_ip, cmd.new_stream_epoch as i64) {
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
                        _ => { continue 'uplink; }
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
                                        stream_epoch: ev.stream_epoch as u64,
                                        seq: ev.seq as u64,
                                        reader_timestamp: ev
                                            .reader_timestamp
                                            .clone()
                                            .unwrap_or_default(),
                                        raw_read_line: ev.raw_read_line.clone(),
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
                    let mut j = journal.lock().await;
                    for entry in &ack.entries {
                        if let Err(e) = j.update_ack_cursor(
                            &entry.reader_ip,
                            entry.stream_epoch as i64,
                            entry.last_seq as i64,
                        ) {
                            warn!(error = %e, "failed to update ack cursor");
                        }
                    }
                }
                Ok(SendBatchResult::EpochReset(cmd)) => {
                    logger.log(format!(
                        "epoch reset for {}; bumping journal",
                        cmd.reader_ip
                    ));
                    let mut j = journal.lock().await;
                    if let Err(e) = j.bump_epoch(&cmd.reader_ip, cmd.new_stream_epoch as i64) {
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

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    // Initialize tracing subscriber for structured logging to stdout.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "forwarder starting");

    // Parse optional --config <path> argument.
    // Defaults to /etc/rusty-timer/forwarder.toml when not supplied.
    let args: Vec<String> = std::env::args().collect();
    let config_path = match args.iter().position(|a| a == "--config") {
        Some(i) => match args.get(i + 1) {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                eprintln!("FATAL: --config requires a path argument");
                std::process::exit(1);
            }
        },
        None => std::path::PathBuf::from("/etc/rusty-timer/forwarder.toml"),
    };

    let cfg = match forwarder::config::load_config_from_path(&config_path) {
        Ok(cfg) => {
            info!(
                base_url = %cfg.server.base_url,
                readers = cfg.readers.len(),
                "config loaded"
            );
            cfg
        }
        Err(e) => {
            eprintln!("FATAL: failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    // Derive forwarder_id from token
    let forwarder_id = derive_forwarder_id(&cfg.token);
    info!(forwarder_id = %forwarder_id, "forwarder identity derived");

    // Open journal
    let journal_path = Path::new(&cfg.journal.sqlite_path);
    let journal = match Journal::open(journal_path) {
        Ok(j) => {
            info!(path = %cfg.journal.sqlite_path, "journal opened");
            Arc::new(Mutex::new(j))
        }
        Err(e) => {
            eprintln!("FATAL: failed to open journal: {}", e);
            std::process::exit(1);
        }
    };

    // Set up shutdown channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Start status HTTP server (not-ready initially)
    let status_cfg = StatusConfig {
        bind: cfg.status_http.bind.clone(),
        forwarder_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let subsystem = SubsystemStatus::not_ready("starting".to_owned());
    let config_state = Arc::new(ConfigState::new(config_path.clone()));
    let restart_signal = Arc::new(Notify::new());
    let status_server = match StatusServer::start_with_config(
        status_cfg,
        subsystem,
        journal.clone(),
        config_state.clone(),
        restart_signal.clone(),
    )
    .await
    {
        Ok(s) => {
            info!(addr = %s.local_addr(), "status HTTP server started");
            s
        }
        Err(e) => {
            eprintln!("FATAL: failed to start status HTTP server: {}", e);
            std::process::exit(1);
        }
    };
    status_server.set_update_mode(cfg.update.mode).await;
    let logger = status_server.logger();

    // Collect enabled reader endpoints
    let mut all_readers: Vec<(String, u16)> = Vec::new(); // (addr, local_port)
    let mut fanout_addrs: Vec<(String, u16, SocketAddr)> = Vec::new(); // (ip, port, fanout_addr)

    for reader_cfg in &cfg.readers {
        if !reader_cfg.enabled {
            info!(target = %reader_cfg.target, "reader disabled, skipping");
            continue;
        }

        let endpoints = match expand_target(&reader_cfg.target) {
            Ok(eps) => eps,
            Err(e) => {
                eprintln!(
                    "FATAL: invalid reader target '{}': {}",
                    reader_cfg.target, e
                );
                std::process::exit(1);
            }
        };

        for ep in endpoints {
            let local_port = reader_cfg
                .local_fallback_port
                .unwrap_or_else(|| ep.default_local_fallback_port());

            let bind_addr = format!("0.0.0.0:{}", local_port);
            let fanout = match FanoutServer::bind(&bind_addr).await {
                Ok(f) => f,
                Err(e) => {
                    eprintln!(
                        "FATAL: failed to bind fanout for {} on port {}: {}",
                        ep.ip, local_port, e
                    );
                    std::process::exit(1);
                }
            };

            let fanout_addr = fanout.local_addr();
            info!(
                reader_ip = %ep.ip,
                local_port = local_port,
                "local fanout listener started"
            );

            // Spawn the fanout accept loop
            tokio::spawn(async move {
                fanout.run().await;
            });

            all_readers.push((ep.addr(), local_port));
            fanout_addrs.push((ep.ip, ep.port, fanout_addr));
        }
    }

    // Initialize reader status tracking
    status_server.init_readers(&all_readers).await;

    if all_readers.is_empty() {
        eprintln!("FATAL: no enabled readers configured");
        std::process::exit(1);
    }

    // Seed historical totals once at startup to avoid per-request DB counting.
    for (reader_addr, _) in &all_readers {
        let total = {
            let j = journal.lock().await;
            match j.event_count(reader_addr) {
                Ok(count) => count,
                Err(e) => {
                    warn!(reader_ip = %reader_addr, error = %e, "failed to load reader total");
                    0
                }
            }
        };
        status_server.set_reader_total(reader_addr, total).await;
    }

    // Set forwarder identity on status page
    status_server.set_forwarder_id(&forwarder_id).await;

    // Detect local IP from first reader
    let local_ip = all_readers.first().and_then(|(addr, _)| {
        let ip = addr.rsplit_once(':').map(|(ip, _)| ip).unwrap_or(addr);
        detect_local_ip(ip)
    });
    if let Some(ref ip) = local_ip {
        info!(local_ip = %ip, "detected local IP");
    }
    status_server.set_local_ip(local_ip).await;

    // Spawn reader tasks
    for (reader_ip, reader_port, fanout_addr) in fanout_addrs {
        let j = journal.clone();
        let rx = shutdown_rx.clone();
        let ss = status_server.clone();
        let lg = logger.clone();
        tokio::spawn(async move {
            run_reader(reader_ip, reader_port, fanout_addr, j, rx, ss, lg).await;
        });
    }

    // Spawn uplink task
    {
        let j = journal.clone();
        let rx = shutdown_rx.clone();
        let fwd_cfg = cfg.clone();
        let fwd_id = forwarder_id.clone();
        let ips: Vec<String> = all_readers.iter().map(|(addr, _)| addr.clone()).collect();
        let ss = status_server.clone();
        let cs = config_state.clone();
        let sub = status_server.subsystem_arc();
        let rs = restart_signal.clone();
        let lg = logger.clone();
        tokio::spawn(async move {
            run_uplink(fwd_cfg, fwd_id, ips, j, rx, ss, cs, sub, rs, lg).await;
        });
    }

    // All worker tasks started — mark subsystem ready
    status_server.set_ready().await;

    if std::env::var_os("RT_UPDATER_STAGE_DIR").is_none() {
        let default_stage_dir = "/var/lib/rusty-timer";
        std::env::set_var("RT_UPDATER_STAGE_DIR", default_stage_dir);
        info!(
            stage_dir = default_stage_dir,
            "configured updater stage directory"
        );
    }

    // Spawn background update check
    {
        let ss = status_server.clone();
        let update_mode = cfg.update.mode;
        let lg = logger.clone();
        tokio::spawn(async move {
            if update_mode == rt_updater::UpdateMode::Disabled {
                lg.log("auto-update disabled by configuration");
                return;
            }

            let checker = match rt_updater::UpdateChecker::new(
                "iwismer",
                "rusty-timer",
                "forwarder",
                env!("CARGO_PKG_VERSION"),
            ) {
                Ok(c) => c,
                Err(e) => {
                    lg.log_at(
                        UiLogLevel::Warn,
                        format!("failed to create update checker: {e}"),
                    );
                    return;
                }
            };

            let status = checker.check().await;
            match status {
                Ok(rt_updater::UpdateStatus::Available { ref version }) => {
                    lg.log(format!("Update v{version} available"));
                    ss.set_update_status(rt_updater::UpdateStatus::Available {
                        version: version.clone(),
                    })
                    .await;

                    if update_mode == rt_updater::UpdateMode::CheckAndDownload {
                        match checker.download(version).await {
                            Ok(path) => {
                                lg.log(format!("Update v{version} downloaded and staged"));
                                ss.set_update_status(rt_updater::UpdateStatus::Downloaded {
                                    version: version.clone(),
                                })
                                .await;
                                ss.set_staged_update_path(path).await;
                            }
                            Err(e) => {
                                lg.log_at(UiLogLevel::Warn, format!("update download failed: {e}"));
                                ss.set_update_status(rt_updater::UpdateStatus::Failed {
                                    error: e.to_string(),
                                })
                                .await;
                            }
                        }
                    }
                }
                Ok(_) => {
                    lg.log("forwarder is up to date");
                }
                Err(e) => {
                    lg.log_at(UiLogLevel::Warn, format!("update check failed: {e}"));
                    ss.set_update_status(rt_updater::UpdateStatus::Failed {
                        error: e.to_string(),
                    })
                    .await;
                }
            }
        });
    }

    logger.log(format!(
        "forwarder v{} initialized — all workers running",
        env!("CARGO_PKG_VERSION")
    ));

    // Wait for Ctrl-C, SIGTERM, or restart request
    let restart_requested;
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                error!("failed to install SIGTERM handler: {}", e);
                tokio::signal::ctrl_c().await.ok();
                shutdown_tx.send(true).ok();
                return;
            }
        };

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                logger.log("shutdown: SIGINT received");
                restart_requested = false;
            }
            _ = sigterm.recv() => {
                logger.log("shutdown: SIGTERM received");
                restart_requested = false;
            }
            _ = restart_signal.notified() => {
                logger.log("restart requested via API");
                restart_requested = true;
            }
        }
    }

    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                logger.log("shutdown: Ctrl-C received");
            }
            _ = restart_signal.notified() => {
                logger.log("restart requested via API");
            }
        }
        restart_requested = false; // exec not available on non-unix
    }

    // Signal all tasks to stop
    shutdown_tx.send(true).ok();

    // Brief delay to allow tasks to observe shutdown and flush
    sleep(Duration::from_millis(200)).await;

    info!("forwarder shutdown complete");

    // Self-exec to restart if requested
    #[cfg(unix)]
    if restart_requested {
        use std::os::unix::process::CommandExt;
        let exe = match std::env::current_exe() {
            Ok(e) => e,
            Err(e) => {
                error!("could not determine executable path: {}", e);
                std::process::exit(1);
            }
        };
        let args: Vec<String> = std::env::args().skip(1).collect();
        info!(exe = %exe.display(), "exec-ing self to restart");
        let err = std::process::Command::new(&exe).args(&args).exec();
        error!("exec failed: {}", err);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use rt_protocol::{EpochResetCommand, Heartbeat, WsMessage};
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio::time::timeout;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::protocol::Message;

    #[tokio::test]
    async fn run_uplink_reconnects_after_replay_epoch_reset_before_main_loop() {
        let reader_ip = "10.0.0.1".to_string();
        let temp_dir = tempdir().expect("create tempdir");
        let db_path = temp_dir.path().join("forwarder.sqlite3");

        let mut journal = Journal::open(&db_path).expect("open journal");
        journal
            .ensure_stream_state(&reader_ip, 1)
            .expect("ensure stream state");
        journal
            .insert_event(
                &reader_ip,
                1,
                1,
                Some("2026-01-01T00:00:00Z"),
                "aa400000000123450a2a01123018455927a7",
                "RAW",
            )
            .expect("insert replay event");

        let journal = Arc::new(Mutex::new(journal));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let (server_result_tx, server_result_rx) = oneshot::channel::<(bool, bool)>();

        let server_task = tokio::spawn(async move {
            let mut sent_extra_batch_on_first_session = false;

            let (stream1, _) = listener.accept().await.expect("accept first session");
            let ws1 = accept_async(stream1)
                .await
                .expect("ws accept first session");
            let (mut write1, mut read1) = ws1.split();

            let hello1 = read1
                .next()
                .await
                .expect("hello on first session")
                .expect("hello frame on first session");
            let hello1 = parse_ws_text_message(hello1);
            let forwarder_id = match hello1 {
                WsMessage::ForwarderHello(h) => h.forwarder_id,
                other => panic!("expected ForwarderHello on first session, got {:?}", other),
            };

            let heartbeat1 = WsMessage::Heartbeat(Heartbeat {
                session_id: "session-1".to_string(),
                device_id: forwarder_id.clone(),
            });
            write1
                .send(Message::Text(
                    serde_json::to_string(&heartbeat1).unwrap().into(),
                ))
                .await
                .expect("send first heartbeat");

            let replay_batch = read1
                .next()
                .await
                .expect("replay batch on first session")
                .expect("replay batch frame on first session");
            let replay_batch = parse_ws_text_message(replay_batch);
            match replay_batch {
                WsMessage::ForwarderEventBatch(batch) => {
                    assert_eq!(batch.events.len(), 1, "expected one replayed event");
                }
                other => panic!("expected replay batch on first session, got {:?}", other),
            }

            let reset = WsMessage::EpochResetCommand(EpochResetCommand {
                session_id: "session-1".to_string(),
                forwarder_id: forwarder_id.clone(),
                reader_ip: "10.0.0.1".to_string(),
                new_stream_epoch: 2,
            });
            write1
                .send(Message::Text(serde_json::to_string(&reset).unwrap().into()))
                .await
                .expect("send epoch reset");

            // Observe first-session traffic until reconnect accept window closes.
            // Any ForwarderEventBatch here is an incorrect same-session fallthrough.
            let second_accept = timeout(std::time::Duration::from_secs(2), listener.accept());
            tokio::pin!(second_accept);

            let mut second_stream = None;
            let mut first_stream_open = true;
            loop {
                tokio::select! {
                    accept_result = &mut second_accept => {
                        if let Ok(Ok((stream2, _))) = accept_result {
                            second_stream = Some(stream2);
                        }
                        break;
                    }
                    maybe_msg = read1.next(), if first_stream_open => {
                        match maybe_msg {
                            Some(Ok(Message::Text(text))) => {
                                let parsed: WsMessage = serde_json::from_str(&text).expect("parse ws json");
                                if let WsMessage::ForwarderEventBatch(_) = parsed {
                                    sent_extra_batch_on_first_session = true;
                                }
                            }
                            Some(Ok(_)) => {}
                            Some(Err(_)) | None => {
                                first_stream_open = false;
                            }
                        }
                    }
                }
            }

            let saw_second_session = second_stream.is_some();
            drop(write1);

            if let Some(stream2) = second_stream {
                let ws2 = accept_async(stream2)
                    .await
                    .expect("ws accept second session");
                let (mut write2, mut read2) = ws2.split();

                let hello2 = read2
                    .next()
                    .await
                    .expect("hello on second session")
                    .expect("hello frame on second session");
                let hello2 = parse_ws_text_message(hello2);
                let second_forwarder_id = match hello2 {
                    WsMessage::ForwarderHello(h) => h.forwarder_id,
                    other => panic!("expected ForwarderHello on second session, got {:?}", other),
                };

                let heartbeat2 = WsMessage::Heartbeat(Heartbeat {
                    session_id: "session-2".to_string(),
                    device_id: second_forwarder_id.clone(),
                });
                write2
                    .send(Message::Text(
                        serde_json::to_string(&heartbeat2).unwrap().into(),
                    ))
                    .await
                    .expect("send second heartbeat");
            }

            let _ = server_result_tx.send((sent_extra_batch_on_first_session, saw_second_session));
        });

        let cfg = ForwarderConfig {
            schema_version: 1,
            token: "test-token".to_string(),
            display_name: None,
            server: forwarder::config::ServerConfig {
                base_url: format!("http://{}", addr),
                forwarders_ws_path: "/ws/v1/forwarders".to_string(),
            },
            journal: forwarder::config::JournalConfig {
                sqlite_path: db_path.display().to_string(),
                prune_watermark_pct: 80,
            },
            status_http: forwarder::config::StatusHttpConfig {
                bind: "127.0.0.1:0".to_string(),
            },
            uplink: forwarder::config::UplinkConfig {
                batch_mode: "immediate".to_string(),
                batch_flush_ms: 50,
                batch_max_events: 50,
            },
            control: forwarder::config::ControlConfig {
                allow_power_actions: false,
            },
            update: forwarder::config::UpdateConfig {
                mode: rt_updater::UpdateMode::default(),
            },
            readers: vec![forwarder::config::ReaderConfig {
                target: reader_ip.clone(),
                enabled: true,
                local_fallback_port: None,
            }],
        };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let status = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start test status server");
        let config_path = temp_dir.path().join("forwarder.toml");
        std::fs::write(&config_path, "schema_version = 1\ntoken = \"test-token\"\n")
            .expect("write test config");
        let config_state = Arc::new(ConfigState::new(config_path));
        let subsystem_arc = status.subsystem_arc();
        let restart_signal = Arc::new(Notify::new());
        let lg = status.logger();
        let uplink_task = tokio::spawn(run_uplink(
            cfg,
            "fwd-epoch-reconnect-test".to_string(),
            vec![reader_ip],
            journal,
            shutdown_rx,
            status,
            config_state,
            subsystem_arc,
            restart_signal,
            lg,
        ));

        let (sent_extra_batch_on_first_session, saw_second_session) =
            timeout(std::time::Duration::from_secs(4), server_result_rx)
                .await
                .expect("server observation timeout")
                .expect("server result");

        assert!(
            !sent_extra_batch_on_first_session,
            "run_uplink sent a batch on the first session after replay EpochReset"
        );
        assert!(
            saw_second_session,
            "run_uplink did not reconnect after replay EpochReset"
        );

        let _ = shutdown_tx.send(true);
        timeout(std::time::Duration::from_secs(2), uplink_task)
            .await
            .expect("uplink task shutdown timeout")
            .expect("uplink task join");
        server_task.await.expect("server task join");
    }

    fn parse_ws_text_message(msg: Message) -> WsMessage {
        match msg {
            Message::Text(text) => serde_json::from_str(&text).expect("parse ws json"),
            other => panic!("expected text ws frame, got {:?}", other),
        }
    }

    #[test]
    fn chunk_for_replay_splits_into_bounded_batches() {
        let events: Vec<ReadEvent> = (1..=5)
            .map(|seq| ReadEvent {
                forwarder_id: "fwd".to_owned(),
                reader_ip: "10.0.0.1".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-01-01T00:00:00Z".to_owned(),
                raw_read_line: format!("LINE_{seq}"),
                read_type: "RAW".to_owned(),
            })
            .collect();

        let chunks = chunk_for_replay(events, 2);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].len(), 2);
        assert_eq!(chunks[1].len(), 2);
        assert_eq!(chunks[2].len(), 1);
    }

    #[test]
    fn chunk_for_replay_treats_zero_max_as_one() {
        let events: Vec<ReadEvent> = (1..=3)
            .map(|seq| ReadEvent {
                forwarder_id: "fwd".to_owned(),
                reader_ip: "10.0.0.1".to_owned(),
                stream_epoch: 1,
                seq,
                reader_timestamp: "2026-01-01T00:00:00Z".to_owned(),
                raw_read_line: format!("LINE_{seq}"),
                read_type: "RAW".to_owned(),
            })
            .collect();

        let chunks = chunk_for_replay(events, 0);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|chunk| chunk.len() == 1));
    }

    #[test]
    fn append_read_to_journal_returns_error_when_stream_missing() {
        let temp_dir = tempdir().expect("create tempdir");
        let db_path = temp_dir.path().join("forwarder.sqlite3");
        let mut journal = Journal::open(&db_path).expect("open journal");

        let result = append_read_to_journal(
            &mut journal,
            "10.0.0.42",
            Some("2026-01-01T00:00:00Z"),
            "aa400000000123450a2a01123018455927a7",
            "raw",
        );

        assert!(
            matches!(result, Err(JournalAppendError::StreamState(_))),
            "missing stream state should return StreamState error, got: {:?}",
            result
        );
    }

    async fn http_get_body(addr: SocketAddr, path: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("connect failed");
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            path
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("write failed");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .await
            .expect("read failed");
        response
    }

    #[tokio::test]
    async fn journal_error_marks_reader_disconnected() {
        let status = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        status
            .init_readers(&[("10.0.0.42".to_owned(), 10042)])
            .await;
        status
            .update_reader_state("10.0.0.42", ReaderConnectionState::Connected)
            .await;

        mark_reader_disconnected(&status, "10.0.0.42").await;

        let body = http_get_body(status.local_addr(), "/api/v1/status").await;
        assert!(
            body.contains("\"state\":\"disconnected\""),
            "reader should be marked disconnected after journal error"
        );
    }

    #[tokio::test]
    async fn run_reader_updates_status_for_ip_port_stream_key() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let reader_port = listener.local_addr().expect("listener local_addr").port();
        let stream_key = format!("127.0.0.1:{reader_port}");

        let status = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");
        status.init_readers(&[(stream_key.clone(), 10001)]).await;

        let temp_dir = tempdir().expect("create tempdir");
        let db_path = temp_dir.path().join("forwarder.sqlite3");
        let journal = Arc::new(Mutex::new(Journal::open(&db_path).expect("open journal")));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let lg = status.logger();
        let reader_task = tokio::spawn(run_reader(
            "127.0.0.1".to_owned(),
            reader_port,
            "127.0.0.1:9".parse().expect("parse fanout addr"),
            journal,
            shutdown_rx,
            status.clone(),
            lg,
        ));

        let (_accepted, _) = timeout(std::time::Duration::from_secs(1), listener.accept())
            .await
            .expect("reader connect timeout")
            .expect("accept reader connection");

        let expected_json = format!("\"ip\":\"{stream_key}\"");
        let expected_state = "\"state\":\"connected\"";
        let mut body = String::new();
        let mut found = false;
        for _ in 0..50 {
            body = http_get_body(status.local_addr(), "/api/v1/status").await;
            if body.contains(&expected_json) && body.contains(expected_state) {
                found = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let _ = shutdown_tx.send(true);
        timeout(std::time::Duration::from_secs(1), reader_task)
            .await
            .expect("reader shutdown timeout")
            .expect("reader task join");

        assert!(
            found,
            "expected connected status row for stream key {stream_key}, body was: {body}"
        );
    }

    #[tokio::test]
    async fn run_reader_journals_detected_read_types() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let reader_port = listener.local_addr().expect("listener local_addr").port();
        let stream_key = format!("127.0.0.1:{reader_port}");

        let status = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");
        status.init_readers(&[(stream_key.clone(), 10001)]).await;

        let temp_dir = tempdir().expect("create tempdir");
        let db_path = temp_dir.path().join("forwarder.sqlite3");
        let journal = Arc::new(Mutex::new(Journal::open(&db_path).expect("open journal")));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let lg = status.logger();
        let reader_task = tokio::spawn(run_reader(
            "127.0.0.1".to_owned(),
            reader_port,
            "127.0.0.1:9".parse().expect("parse fanout addr"),
            journal.clone(),
            shutdown_rx,
            status,
            lg,
        ));

        let (mut reader_stream, _) = timeout(std::time::Duration::from_secs(1), listener.accept())
            .await
            .expect("reader connect timeout")
            .expect("accept reader connection");

        reader_stream
            .write_all(b"aa400000000123450a2a01123018455927a7\n")
            .await
            .expect("write raw read");
        reader_stream
            .write_all(b"aa400000000123450a2a01123018455927a7FS\n")
            .await
            .expect("write fsls read");
        drop(reader_stream);

        let mut events = Vec::new();
        for _ in 0..50 {
            {
                let j = journal.lock().await;
                events = j
                    .unacked_events(&stream_key, 1, 0)
                    .expect("read journal events");
            }
            if events.len() >= 2 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let _ = shutdown_tx.send(true);
        timeout(std::time::Duration::from_secs(1), reader_task)
            .await
            .expect("reader shutdown timeout")
            .expect("reader task join");

        assert_eq!(
            events.len(),
            2,
            "expected 2 events in journal, got {}",
            events.len()
        );
        assert_eq!(events[0].read_type, "raw");
        assert_eq!(events[1].read_type, "fsls");
    }

    #[tokio::test]
    async fn config_get_set_over_websocket() {
        // 1. Set up tempdir, write a minimal TOML config file
        let temp_dir = tempdir().expect("create tempdir");
        let db_path = temp_dir.path().join("forwarder.sqlite3");
        let config_path = temp_dir.path().join("forwarder.toml");
        std::fs::write(
            &config_path,
            r#"
schema_version = 1
display_name = "Test Forwarder"

[server]
base_url = "http://localhost:9999"

[auth]
token_file = "/tmp/test-token"
"#,
        )
        .expect("write test config");

        // 2. Open journal
        let journal = Arc::new(Mutex::new(Journal::open(&db_path).expect("open journal")));

        // 3. Bind mock server
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");

        // 4. Use a oneshot channel to collect test results from server task
        let (result_tx, result_rx) = oneshot::channel::<(
            serde_json::Value, // config from ConfigGetResponse
            bool,              // restart_needed from ConfigGetResponse
            bool,              // ok from ConfigSetResponse (valid section)
            bool,              // ok from ConfigSetResponse (invalid section)
            Option<String>,    // error from ConfigSetResponse (invalid section)
        )>();

        // 5. Server task: accept WS, do hello/heartbeat handshake, then test config messages
        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let ws = accept_async(stream).await.expect("ws accept");
            let (mut write, mut read) = ws.split();

            // Receive forwarder_hello
            let hello = read.next().await.expect("hello").expect("hello frame");
            let hello = parse_ws_text_message(hello);
            let forwarder_id = match hello {
                WsMessage::ForwarderHello(h) => h.forwarder_id,
                other => panic!("expected ForwarderHello, got {:?}", other),
            };

            // Send heartbeat
            let hb = WsMessage::Heartbeat(Heartbeat {
                session_id: "test-session".to_string(),
                device_id: forwarder_id.clone(),
            });
            write
                .send(Message::Text(serde_json::to_string(&hb).unwrap().into()))
                .await
                .expect("send heartbeat");

            // Wait a moment for the forwarder to enter its idle polling loop
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            // --- Test 1: ConfigGetRequest ---
            let get_req = WsMessage::ConfigGetRequest(rt_protocol::ConfigGetRequest {
                request_id: "test-get-1".to_string(),
            });
            write
                .send(Message::Text(
                    serde_json::to_string(&get_req).unwrap().into(),
                ))
                .await
                .expect("send config get request");

            // Receive ConfigGetResponse
            let get_resp = timeout(std::time::Duration::from_secs(5), read.next())
                .await
                .expect("get response timeout")
                .expect("get response")
                .expect("get response frame");
            let get_resp = parse_ws_text_message(get_resp);
            let (config, restart_needed) = match get_resp {
                WsMessage::ConfigGetResponse(r) => {
                    assert_eq!(r.request_id, "test-get-1");
                    (r.config, r.restart_needed)
                }
                other => panic!("expected ConfigGetResponse, got {:?}", other),
            };

            // --- Test 2: ConfigSetRequest (valid section) ---
            let set_req = WsMessage::ConfigSetRequest(rt_protocol::ConfigSetRequest {
                request_id: "test-set-1".to_string(),
                section: "general".to_string(),
                payload: serde_json::json!({ "display_name": "Updated Name" }),
            });
            write
                .send(Message::Text(
                    serde_json::to_string(&set_req).unwrap().into(),
                ))
                .await
                .expect("send config set request");

            let set_resp = timeout(std::time::Duration::from_secs(5), read.next())
                .await
                .expect("set response timeout")
                .expect("set response")
                .expect("set response frame");
            let set_resp = parse_ws_text_message(set_resp);
            let set_ok = match set_resp {
                WsMessage::ConfigSetResponse(r) => {
                    assert_eq!(r.request_id, "test-set-1");
                    r.ok
                }
                other => panic!("expected ConfigSetResponse, got {:?}", other),
            };

            // --- Test 3: ConfigSetRequest (invalid section) ---
            let bad_req = WsMessage::ConfigSetRequest(rt_protocol::ConfigSetRequest {
                request_id: "test-set-2".to_string(),
                section: "nonexistent".to_string(),
                payload: serde_json::json!({}),
            });
            write
                .send(Message::Text(
                    serde_json::to_string(&bad_req).unwrap().into(),
                ))
                .await
                .expect("send bad config set request");

            let bad_resp = timeout(std::time::Duration::from_secs(5), read.next())
                .await
                .expect("bad set response timeout")
                .expect("bad set response")
                .expect("bad set response frame");
            let bad_resp = parse_ws_text_message(bad_resp);
            let (bad_ok, bad_error) = match bad_resp {
                WsMessage::ConfigSetResponse(r) => {
                    assert_eq!(r.request_id, "test-set-2");
                    (r.ok, r.error)
                }
                other => panic!(
                    "expected ConfigSetResponse for bad section, got {:?}",
                    other
                ),
            };

            let _ = result_tx.send((config, restart_needed, set_ok, bad_ok, bad_error));
        });

        // 6. Create ForwarderConfig and spawn run_uplink
        let cfg = ForwarderConfig {
            schema_version: 1,
            token: "test-token".to_string(),
            display_name: Some("Test Forwarder".to_string()),
            server: forwarder::config::ServerConfig {
                base_url: format!("http://{}", addr),
                forwarders_ws_path: "/ws/v1/forwarders".to_string(),
            },
            journal: forwarder::config::JournalConfig {
                sqlite_path: db_path.display().to_string(),
                prune_watermark_pct: 80,
            },
            status_http: forwarder::config::StatusHttpConfig {
                bind: "127.0.0.1:0".to_string(),
            },
            uplink: forwarder::config::UplinkConfig {
                batch_mode: "immediate".to_string(),
                batch_flush_ms: 50,
                batch_max_events: 50,
            },
            control: forwarder::config::ControlConfig {
                allow_power_actions: false,
            },
            update: forwarder::config::UpdateConfig {
                mode: rt_updater::UpdateMode::default(),
            },
            readers: vec![],
        };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let status = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start test status server");
        let config_state = Arc::new(ConfigState::new(config_path));
        let subsystem_arc = status.subsystem_arc();
        let restart_signal = Arc::new(Notify::new());
        let lg = status.logger();
        let uplink_task = tokio::spawn(run_uplink(
            cfg,
            "fwd-config-test".to_string(),
            vec![],
            journal,
            shutdown_rx,
            status,
            config_state,
            subsystem_arc,
            restart_signal,
            lg,
        ));

        // 7. Wait for results
        let (config, restart_needed, set_ok, bad_ok, bad_error) =
            timeout(std::time::Duration::from_secs(10), result_rx)
                .await
                .expect("test observation timeout")
                .expect("test result");

        // 8. Assert config get response
        assert!(!config.is_null(), "config should not be null");
        assert_eq!(
            config.get("display_name").and_then(|v| v.as_str()),
            Some("Test Forwarder"),
            "config should contain display_name"
        );
        assert!(!restart_needed, "restart_needed should be false initially");

        // 9. Assert valid config set response
        assert!(
            set_ok,
            "config set for valid section should return ok: true"
        );

        // 10. Assert invalid config set response
        assert!(
            !bad_ok,
            "config set for invalid section should return ok: false"
        );
        assert!(
            bad_error.is_some(),
            "config set for invalid section should return an error message"
        );

        // 11. Cleanup
        let _ = shutdown_tx.send(true);
        timeout(std::time::Duration::from_secs(2), uplink_task)
            .await
            .expect("uplink task shutdown timeout")
            .expect("uplink task join");
        server_task.await.expect("server task join");
    }

    #[tokio::test]
    async fn run_uplink_does_not_resend_batch_after_config_get_then_ack() {
        let reader_ip = "10.0.0.9:10000".to_string();
        let temp_dir = tempdir().expect("create tempdir");
        let db_path = temp_dir.path().join("forwarder.sqlite3");
        let config_path = temp_dir.path().join("forwarder.toml");

        std::fs::write(
            &config_path,
            r#"
schema_version = 1
display_name = "Ack Test"

[server]
base_url = "http://localhost:9999"

[auth]
token_file = "/tmp/test-token"
"#,
        )
        .expect("write test config");

        let mut journal = Journal::open(&db_path).expect("open journal");
        journal
            .ensure_stream_state(&reader_ip, 1)
            .expect("ensure stream state");
        let journal = Arc::new(Mutex::new(journal));
        let journal_for_inject = journal.clone();

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let (result_tx, result_rx) = oneshot::channel::<bool>();
        let (ready_tx, ready_rx) = oneshot::channel::<()>();
        let reader_ip_for_server = reader_ip.clone();

        let server_task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let ws = accept_async(stream).await.expect("ws accept");
            let (mut write, mut read) = ws.split();

            let hello = read.next().await.expect("hello").expect("hello frame");
            let hello = parse_ws_text_message(hello);
            let forwarder_id = match hello {
                WsMessage::ForwarderHello(h) => h.forwarder_id,
                other => panic!("expected ForwarderHello, got {:?}", other),
            };

            let hb = WsMessage::Heartbeat(Heartbeat {
                session_id: "ack-test-session".to_string(),
                device_id: forwarder_id.clone(),
            });
            write
                .send(Message::Text(serde_json::to_string(&hb).unwrap().into()))
                .await
                .expect("send heartbeat");
            let _ = ready_tx.send(());

            let first_batch = timeout(std::time::Duration::from_secs(3), read.next())
                .await
                .expect("first batch timeout")
                .expect("first batch")
                .expect("first batch frame");
            let first_batch = parse_ws_text_message(first_batch);
            match first_batch {
                WsMessage::ForwarderEventBatch(batch) => {
                    assert_eq!(batch.events.len(), 1);
                    assert_eq!(batch.events[0].reader_ip, reader_ip_for_server);
                    assert_eq!(batch.events[0].seq, 1);
                }
                other => panic!("expected ForwarderEventBatch, got {:?}", other),
            }

            let get_req = WsMessage::ConfigGetRequest(rt_protocol::ConfigGetRequest {
                request_id: "cfg-ack-interleave".to_string(),
            });
            write
                .send(Message::Text(
                    serde_json::to_string(&get_req).unwrap().into(),
                ))
                .await
                .expect("send config get request");

            let get_resp = read
                .next()
                .await
                .expect("config get response")
                .expect("config get response frame");
            let get_resp = parse_ws_text_message(get_resp);
            match get_resp {
                WsMessage::ConfigGetResponse(resp) => {
                    assert_eq!(resp.request_id, "cfg-ack-interleave");
                }
                other => panic!("expected ConfigGetResponse, got {:?}", other),
            }

            let ack = WsMessage::ForwarderAck(rt_protocol::ForwarderAck {
                session_id: "ack-test-session".to_string(),
                entries: vec![rt_protocol::AckEntry {
                    forwarder_id: forwarder_id.clone(),
                    reader_ip: reader_ip_for_server.clone(),
                    stream_epoch: 1,
                    last_seq: 1,
                }],
            });
            write
                .send(Message::Text(serde_json::to_string(&ack).unwrap().into()))
                .await
                .expect("send ack");

            let mut resent = false;
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(400);
            loop {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                let remaining = deadline.saturating_duration_since(now);
                match timeout(remaining, read.next()).await {
                    Ok(Some(Ok(msg))) => {
                        let parsed = parse_ws_text_message(msg);
                        if let WsMessage::ForwarderEventBatch(_) = parsed {
                            resent = true;
                            break;
                        }
                    }
                    Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
                }
            }

            let _ = result_tx.send(resent);
        });

        let cfg = ForwarderConfig {
            schema_version: 1,
            token: "test-token".to_string(),
            display_name: Some("Ack Test".to_string()),
            server: forwarder::config::ServerConfig {
                base_url: format!("http://{}", addr),
                forwarders_ws_path: "/ws/v1/forwarders".to_string(),
            },
            journal: forwarder::config::JournalConfig {
                sqlite_path: db_path.display().to_string(),
                prune_watermark_pct: 80,
            },
            status_http: forwarder::config::StatusHttpConfig {
                bind: "127.0.0.1:0".to_string(),
            },
            uplink: forwarder::config::UplinkConfig {
                batch_mode: "immediate".to_string(),
                batch_flush_ms: 50,
                batch_max_events: 50,
            },
            control: forwarder::config::ControlConfig {
                allow_power_actions: false,
            },
            update: forwarder::config::UpdateConfig {
                mode: rt_updater::UpdateMode::default(),
            },
            readers: vec![forwarder::config::ReaderConfig {
                target: reader_ip.clone(),
                enabled: true,
                local_fallback_port: None,
            }],
        };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let status = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start test status server");

        let config_state = Arc::new(ConfigState::new(config_path));
        let subsystem_arc = status.subsystem_arc();
        let restart_signal = Arc::new(Notify::new());
        let lg = status.logger();

        let uplink_task = tokio::spawn(run_uplink(
            cfg,
            "fwd-ack-interleave".to_string(),
            vec![reader_ip.clone()],
            journal,
            shutdown_rx,
            status,
            config_state,
            subsystem_arc,
            restart_signal,
            lg,
        ));

        timeout(std::time::Duration::from_secs(2), ready_rx)
            .await
            .expect("handshake ready timeout")
            .expect("handshake ready");
        {
            let mut j = journal_for_inject.lock().await;
            j.insert_event(
                &reader_ip,
                1,
                1,
                Some("2026-01-01T00:00:00Z"),
                "aa400000000123450a2a01123018455927a7",
                "RAW",
            )
            .expect("insert pending event");
        }

        let resent = timeout(std::time::Duration::from_secs(3), result_rx)
            .await
            .expect("result timeout")
            .expect("result value");

        let _ = shutdown_tx.send(true);
        timeout(std::time::Duration::from_secs(2), uplink_task)
            .await
            .expect("uplink shutdown timeout")
            .expect("uplink join");
        server_task.await.expect("server task join");

        assert!(
            !resent,
            "forwarder should not resend batch after config-get interleaving once ack is sent"
        );
    }

    #[tokio::test]
    async fn run_uplink_emits_ui_log_entry_when_connect_fails() {
        let temp_dir = tempdir().expect("create tempdir");
        let db_path = temp_dir.path().join("forwarder.sqlite3");
        let config_path = temp_dir.path().join("forwarder.toml");

        std::fs::write(&config_path, "schema_version = 1\ntoken = \"test-token\"\n")
            .expect("write test config");

        let journal = Arc::new(Mutex::new(Journal::open(&db_path).expect("open journal")));
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let unused_port = listener.local_addr().expect("local_addr").port();
        drop(listener);

        let cfg = ForwarderConfig {
            schema_version: 1,
            token: "test-token".to_string(),
            display_name: Some("Connect Failure Test".to_string()),
            server: forwarder::config::ServerConfig {
                base_url: format!("http://127.0.0.1:{unused_port}"),
                forwarders_ws_path: "/ws/v1/forwarders".to_string(),
            },
            journal: forwarder::config::JournalConfig {
                sqlite_path: db_path.display().to_string(),
                prune_watermark_pct: 80,
            },
            status_http: forwarder::config::StatusHttpConfig {
                bind: "127.0.0.1:0".to_string(),
            },
            uplink: forwarder::config::UplinkConfig {
                batch_mode: "immediate".to_string(),
                batch_flush_ms: 50,
                batch_max_events: 50,
            },
            control: forwarder::config::ControlConfig {
                allow_power_actions: false,
            },
            update: forwarder::config::UpdateConfig {
                mode: rt_updater::UpdateMode::default(),
            },
            readers: vec![],
        };

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let status = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start test status server");
        let mut ui_rx = status.ui_sender().subscribe();

        let config_state = Arc::new(ConfigState::new(config_path));
        let subsystem_arc = status.subsystem_arc();
        let restart_signal = Arc::new(Notify::new());
        let lg = status.logger();
        let uplink_task = tokio::spawn(run_uplink(
            cfg,
            "fwd-connect-failure-test".to_string(),
            vec![],
            journal,
            shutdown_rx,
            status,
            config_state,
            subsystem_arc,
            restart_signal,
            lg,
        ));

        let log_entry = timeout(std::time::Duration::from_secs(2), async {
            loop {
                match ui_rx.recv().await.expect("recv ui event") {
                    forwarder::ui_events::ForwarderUiEvent::LogEntry { entry }
                        if entry.contains("uplink connect failed") =>
                    {
                        break entry
                    }
                    _ => continue,
                }
            }
        })
        .await
        .expect("expected ui log entry on connect failure");

        assert!(
            log_entry.contains("uplink connect failed"),
            "expected uplink connect failure log entry, got: {log_entry}"
        );

        let _ = shutdown_tx.send(true);
        timeout(std::time::Duration::from_secs(2), uplink_task)
            .await
            .expect("uplink task shutdown timeout")
            .expect("uplink task join");
    }
}
