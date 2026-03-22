use crate::Subscription;
use crate::cache::EventBus;
use crate::control_api::{AppState, ConnectionState, ShutdownSignal};
use crate::db::Db;
use crate::local_proxy::LocalProxy;
use crate::ports::{PortAssignment, resolve_ports, stream_key};
use rt_ui_log::UiLogLevel;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::watch;
use tokio_tungstenite::connect_async;
use tracing::{error, info, warn};

pub fn generate_receiver_id() -> String {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).expect("failed to generate random bytes");
    format!("recv-{:08x}", u32::from_be_bytes(bytes))
}

pub fn resolve_receiver_id(cli_id: Option<String>, db: &Db) -> Result<String, String> {
    // CLI flag takes priority
    if let Some(id) = cli_id
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    {
        if id.len() > 64
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(
                "receiver_id must be 1-64 characters, alphanumeric/hyphens/underscores only"
                    .to_owned(),
            );
        }
        if let Err(e) = db.save_receiver_id(&id) {
            warn!(error = %e, "failed to persist CLI receiver_id to DB");
        }
        return Ok(id);
    }

    // DB lookup
    match db.load_profile() {
        Ok(Some(p)) => {
            if let Some(id) = p.receiver_id.filter(|id| !id.is_empty()) {
                return Ok(id);
            }
        }
        Ok(None) => {}
        Err(e) => {
            let id = generate_receiver_id();
            warn!(error = %e, receiver_id = %id, "failed to load profile; using ephemeral receiver ID");
            return Ok(id);
        }
    }

    // Auto-generate
    let id = generate_receiver_id();
    if let Err(e) = db.save_receiver_id(&id) {
        warn!(error = %e, "failed to persist auto-generated receiver ID; ID will not survive restart");
    }
    info!(receiver_id = %id, "auto-generated receiver ID");
    Ok(id)
}

pub fn profile_has_connect_credentials(profile: Option<&crate::db::Profile>) -> bool {
    profile.is_some_and(|profile| {
        !profile.server_url.trim().is_empty() && !profile.token.trim().is_empty()
    })
}

/// Initialize the receiver: open DB, create AppState, restore profile/subscriptions.
/// Returns the state, a shutdown receiver, and whether auto-connect should happen.
pub async fn init(
    receiver_id: Option<String>,
) -> Result<(Arc<AppState>, watch::Receiver<ShutdownSignal>), String> {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rusty-timer")
        .join("receiver");

    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("could not create data directory: {e}"))?;

    let db_path = data_dir.join("receiver.sqlite3");
    let db = Db::open(&db_path).map_err(|e| format!("failed to open DB: {e}"))?;
    let db_integrity_ok = db
        .integrity_check()
        .map(|()| true)
        .map_err(|e| format!("integrity_check failed: {e}"))?;

    let receiver_id = resolve_receiver_id(receiver_id, &db)?;
    info!(receiver_id = %receiver_id, "resolved receiver ID");

    let (state, shutdown_rx) = AppState::with_integrity(db, receiver_id, db_integrity_ok);
    state.logger.log("Receiver started");

    Ok((state, shutdown_rx))
}

/// Stop a running DBF writer task, waiting up to 2 seconds before aborting.
async fn stop_dbf_writer(
    cancel_tx: Option<watch::Sender<bool>>,
    cancel_flag: Option<Arc<AtomicBool>>,
    task: Option<tokio::task::JoinHandle<()>>,
) {
    if let Some(cancel_flag) = cancel_flag {
        cancel_flag.store(true, Ordering::SeqCst);
    }
    if let Some(cancel_tx) = cancel_tx {
        let _ = cancel_tx.send(true);
    }
    if let Some(handle) = task {
        tokio::pin!(handle);
        match tokio::time::timeout(std::time::Duration::from_secs(2), &mut handle).await {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) => {
                tracing::error!(error = %join_err, "DBF writer task panicked");
            }
            Err(_elapsed) => {
                tracing::error!("DBF writer task did not shut down within 2 seconds");
                handle.abort();
            }
        }
    }
}

/// Run the receiver event loop. Blocks until shutdown signal.
/// This should be spawned as a tokio task, not run on the main thread.
pub async fn run(state: Arc<AppState>, mut shutdown_rx: watch::Receiver<ShutdownSignal>) {
    // Load profile and restore subscriptions
    let has_profile: bool;
    {
        let db = state.db.lock().await;
        let profile = db.load_profile().ok().flatten();
        has_profile = profile_has_connect_credentials(profile.as_ref());
        if let Some(ref p) = profile {
            *state.upstream_url.write().await = Some(p.server_url.clone());
            info!(url = %p.server_url, "restored profile");
        }
    }

    let event_bus = EventBus::new();
    let (global_event_tx, _global_event_rx) =
        tokio::sync::broadcast::channel::<rt_protocol::ReadEvent>(256);

    // Start local proxies for any saved subscriptions on startup.
    let initial_subs = {
        let db = state.db.lock().await;
        match db.load_subscriptions() {
            Ok(subs) => subs,
            Err(e) => {
                warn!(error = %e, "failed to load subscriptions at startup; starting with none");
                vec![]
            }
        }
    };
    let mut proxies: HashMap<String, LocalProxy> = HashMap::new();
    reconcile_proxies(&initial_subs, &mut proxies, &event_bus, &state.logger).await;

    // Auto-connect on startup if a profile with URL and token exists.
    if has_profile {
        state.logger.log("Auto-connecting to server");
        state.request_connect().await;
    }

    let mut dbf_writer_cancel_tx: Option<watch::Sender<bool>> = None;
    let mut dbf_writer_cancel_flag: Option<Arc<AtomicBool>> = None;
    let mut dbf_writer_task: Option<tokio::task::JoinHandle<()>> = None;
    {
        let db = state.db.lock().await;
        match db.load_dbf_config() {
            Ok(dbf_config) if dbf_config.enabled => {
                let (cancel_tx, cancel_rx) = watch::channel(false);
                let cancel_flag = Arc::new(AtomicBool::new(false));
                let rx = global_event_tx.subscribe();
                let db_arc = Arc::clone(&state.db);
                let path = dbf_config.path.clone();
                let ui = state.ui_tx.clone();
                let cancel_flag_for_task = Arc::clone(&cancel_flag);
                let handle = tokio::spawn(async move {
                    crate::dbf_writer::run_dbf_writer(
                        rx,
                        db_arc,
                        cancel_rx,
                        cancel_flag_for_task,
                        path,
                        ui,
                    )
                    .await;
                });
                dbf_writer_cancel_tx = Some(cancel_tx);
                dbf_writer_cancel_flag = Some(cancel_flag);
                dbf_writer_task = Some(handle);
                state.logger.log("DBF writer started");
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!(error = %e, "failed to load DBF config at startup, DBF writer will not start");
                state.logger.log(format!("DBF writer failed to start: {e}"));
            }
        }
    }

    // Spawn upstream dashboard SSE refresher.
    // Uses tokio::spawn (not tauri::async_runtime::spawn) because this is a
    // library crate with no Tauri dependency — the caller spawns `run()` onto
    // the Tauri async runtime, so tokio::spawn here runs on the same executor.
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            run_upstream_dashboard_sse_refresher(state).await;
        });
    }

    // -------------------------------------------------------------------------
    // Event loop: watch connection_state + reconcile subscriptions
    // -------------------------------------------------------------------------
    let mut session_cancel_tx: Option<watch::Sender<bool>> = None;
    let mut session_task: Option<tokio::task::JoinHandle<()>> = None;

    let mut conn_state_rx = state.conn_rx();
    let mut dbf_config_rx = state.dbf_config_rx();

    let mut reconcile_interval = tokio::time::interval(std::time::Duration::from_millis(500));
    reconcile_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_subs: Vec<Subscription> = initial_subs;

    loop {
        tokio::select! {
            biased;

            result = shutdown_rx.changed() => {
                if result.is_err() {
                    info!("shutdown channel closed, exiting");
                    break;
                }
                if should_exit_on_shutdown_signal(&shutdown_rx.borrow()) {
                    info!("process shutdown requested");
                    break;
                }
            }

            _ = reconcile_interval.tick() => {
                let current_subs = {
                    let db = state.db.lock().await;
                    db.load_subscriptions().unwrap_or_default()
                };
                if current_subs != last_subs {
                    reconcile_proxies(&current_subs, &mut proxies, &event_bus, &state.logger).await;
                    let keep: HashSet<crate::StreamKey> = current_subs
                        .iter()
                        .map(|s| crate::StreamKey::new(&s.forwarder_id, &s.reader_ip))
                        .collect();
                    state.stream_counts.retain_keys(&keep);
                    state.logger.log(format!("Subscriptions changed ({} streams)", current_subs.len()));
                    state.emit_streams_snapshot().await;
                    last_subs = current_subs;
                }
            }

            result = watch_connection_state(&mut conn_state_rx) => {
                match result {
                    ConnectionState::Connecting => {
                        let attempt = state.current_connect_attempt();

                        let retries = state.current_retry_streak();
                        let delay_secs = compute_reconnect_delay_secs(retries);
                        if delay_secs > 0 {
                            state.logger.log(format!("Reconnecting in {delay_secs}s"));
                        }
                        if !wait_for_reconnect_delay_or_abort(&state, attempt, retries).await {
                            continue;
                        }

                        cancel_session(&mut session_task, &mut session_cancel_tx, &state.logger).await;

                        let url_opt = state.upstream_url.read().await.clone();
                        match url_opt {
                            None => {
                                state.logger.log_at(UiLogLevel::Warn, "No upstream URL configured");
                                let _ =
                                    set_disconnected_if_attempt_current(&state, attempt).await;
                            }
                            Some(base_url) => {
                                let ws_url = format!(
                                    "{}/ws/v1.2/receivers",
                                    base_url.trim_end_matches('/')
                                );
                                let token_opt = {
                                    let db = state.db.lock().await;
                                    db.load_profile().ok().flatten().map(|p| p.token)
                                };
                                match token_opt {
                                  None => {
                                    state.logger.log_at(UiLogLevel::Warn, "No auth token in profile");
                                    let _ = set_disconnected_if_attempt_current(&state, attempt)
                                        .await;
                                  }
                                  Some(token) => {
                                let ws_request =
                                    crate::build_authenticated_request(ws_url.as_str(), &token);
                                match ws_request {
                                  Err(e) => {
                                    state.logger.log_at(UiLogLevel::Error, format!("Failed to build WS request: {e}"));
                                    let _ = set_disconnected_if_attempt_current(&state, attempt)
                                        .await;
                                  }
                                  Ok(ws_request) => {
                                match connect_async(ws_request).await {
                                    Err(e) => {
                                        state.logger.log_at(UiLogLevel::Error, format!("Connection failed: {e}"));
                                        let _ = retry_connect_if_attempt_current(&state, attempt)
                                            .await;
                                    }
                                    Ok((ws, _)) => {
                                        let (session_result, ws) = {
                                            let receiver_id = state.receiver_id.read().await.clone();
                                            do_handshake(ws, &state.db, &state.ui_tx, &receiver_id, &state.http_client).await
                                        };
                                        match (session_result, ws) {
                                            (Err(e), _) => {
                                                state.logger.log_at(UiLogLevel::Error, format!("Handshake failed: {e}"));
                                                if matches!(e, crate::session::SessionError::ServerError(_)) {
                                                    let _ = set_disconnected_if_attempt_current(&state, attempt).await;
                                                } else {
                                                    let _ = retry_connect_if_attempt_current(
                                                        &state, attempt,
                                                    )
                                                    .await;
                                                }
                                            }
                                            (Ok(session_id), Some(ws)) => {
                                                let still_current =
                                                    is_current_connect_attempt(&state, attempt).await;
                                                if !still_current {
                                                    state.logger.log("Discarding stale connect attempt");
                                                    continue;
                                                }
                                                state.reset_retry_streak();
                                                state.logger.log(format!("Connected (session {session_id})"));
                                                state.set_connection_state(ConnectionState::Connected).await;
                                                state.emit_streams_snapshot().await;

                                                refresh_chip_lookup(&state).await;

                                                let (cancel_tx, cancel_rx) =
                                                    watch::channel(false);
                                                let db_arc = Arc::clone(&state.db);
                                                let bus = event_bus.clone();
                                                let counts = state.stream_counts.clone();
                                                let ui_tx = state.ui_tx.clone();
                                                let (ws_cmd_tx, ws_cmd_rx) =
                                                    tokio::sync::mpsc::channel(16);
                                                {
                                                    let mut guard = state.ws_cmd_tx.write().await;
                                                    *guard = Some(ws_cmd_tx);
                                                }
                                                let st = Arc::clone(&state);
                                                let gtx = global_event_tx.clone();
                                                let handle = tokio::spawn(async move {
                                                    let event_tx = make_broadcast_sender(&bus);
                                                    let deps = crate::session::SessionLoopDeps {
                                                        db: db_arc,
                                                        event_tx,
                                                        dbf_event_tx: Some(gtx),
                                                        stream_counts: counts,
                                                        ui_tx,
                                                        shutdown: cancel_rx,
                                                        connection_state: st.conn_rx(),
                                                        chip_lookup: Arc::clone(&st.chip_lookup),
                                                        ws_cmd_rx,
                                                    };
                                                    let result = crate::session::run_session_loop(
                                                        ws, session_id, deps,
                                                    )
                                                    .await;
                                                    // Clear the WS command sender so Tauri commands
                                                    // know the session is gone.
                                                    {
                                                        let mut guard = st.ws_cmd_tx.write().await;
                                                        *guard = None;
                                                    }
                                                    match result {
                                                        Ok(()) => {
                                                            info!("WS session ended normally");
                                                        }
                                                        Err(e) => {
                                                            st.logger.log_at(
                                                                UiLogLevel::Error,
                                                                format!("WS session error: {e}"),
                                                            );
                                                        }
                                                    }
                                                    if request_reconnect_if_connected(&st).await {
                                                        st.logger.log("Connection lost, will reconnect");
                                                        st.emit_streams_snapshot().await;
                                                    } else if *st.connection_state.borrow()
                                                        == ConnectionState::Disconnecting
                                                    {
                                                        st.set_connection_state(ConnectionState::Disconnected).await;
                                                        st.emit_streams_snapshot().await;
                                                    }
                                                });
                                                session_task = Some(handle);
                                                session_cancel_tx = Some(cancel_tx);
                                            }
                                            (Ok(_), None) => {
                                                state.logger.log_at(UiLogLevel::Error, "Handshake succeeded but connection lost");
                                                let _ = retry_connect_if_attempt_current(
                                                    &state, attempt,
                                                )
                                                .await;
                                            }
                                        }
                                    }
                                }       // match connect_async
                                  }     // Ok(ws_request) =>
                                }       // match ws_request
                              }         // Some(token) =>
                            }           // match token_opt
                        }               // Some(url) =>
                    }                   // match url_opt
                }                       // ConnectionState::Connecting =>

                    ConnectionState::Disconnecting => {
                        info!("disconnecting: cancelling WS session");
                        cancel_session(&mut session_task, &mut session_cancel_tx, &state.logger).await;
                        state.set_connection_state(ConnectionState::Disconnected).await;
                        state.emit_streams_snapshot().await;
                        info!("disconnected (local proxies remain open)");
                    }

                    _ => {
                        // Disconnected / Connected — no action needed.
                    }
                }
            }

            _ = dbf_config_rx.changed() => {
                let db = state.db.lock().await;
                let dbf_config = match db.load_dbf_config() {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!(error = %e, "failed to reload DBF config, keeping current state");
                        continue;
                    }
                };
                drop(db);

                // Stop existing writer if running
                stop_dbf_writer(
                    dbf_writer_cancel_tx.take(),
                    dbf_writer_cancel_flag.take(),
                    dbf_writer_task.take(),
                )
                .await;

                // Start new writer if enabled
                if dbf_config.enabled {
                    let (cancel_tx, cancel_rx) = watch::channel(false);
                    let cancel_flag = Arc::new(AtomicBool::new(false));
                    let rx = global_event_tx.subscribe();
                    let db_arc = Arc::clone(&state.db);
                    let path = dbf_config.path.clone();
                    let ui = state.ui_tx.clone();
                    let cancel_flag_for_task = Arc::clone(&cancel_flag);
                    let handle = tokio::spawn(async move {
                        crate::dbf_writer::run_dbf_writer(
                            rx,
                            db_arc,
                            cancel_rx,
                            cancel_flag_for_task,
                            path,
                            ui,
                        )
                        .await;
                    });
                    dbf_writer_cancel_tx = Some(cancel_tx);
                    dbf_writer_cancel_flag = Some(cancel_flag);
                    dbf_writer_task = Some(handle);
                    state.logger.log("DBF writer started");
                } else {
                    state.logger.log("DBF writer stopped");
                }
            }
        }
    }

    // Graceful shutdown
    state.logger.log("shutdown signal received");
    cancel_session(&mut session_task, &mut session_cancel_tx, &state.logger).await;
    stop_dbf_writer(
        dbf_writer_cancel_tx.take(),
        dbf_writer_cancel_flag.take(),
        dbf_writer_task.take(),
    )
    .await;
    for (key, proxy) in proxies.drain() {
        info!(key = %key, port = proxy.port, "closing local proxy");
        proxy.shutdown();
    }
    info!("receiver stopped");
}

pub fn should_exit_on_shutdown_signal(signal: &ShutdownSignal) -> bool {
    matches!(signal, ShutdownSignal::Terminate)
}

pub async fn watch_connection_state(rx: &mut watch::Receiver<ConnectionState>) -> ConnectionState {
    let current = rx.borrow().clone();
    if current == ConnectionState::Connecting || current == ConnectionState::Disconnecting {
        return current;
    }

    loop {
        rx.changed().await.expect("watch sender dropped");
        let cs = rx.borrow().clone();
        if cs == ConnectionState::Connecting || cs == ConnectionState::Disconnecting {
            return cs;
        }
    }
}

pub async fn is_current_connect_attempt(state: &Arc<AppState>, attempt: u64) -> bool {
    state.current_connect_attempt() == attempt
        && *state.connection_state.borrow() == ConnectionState::Connecting
}

pub fn compute_reconnect_delay_secs(retries: u64) -> u64 {
    if retries == 0 {
        0
    } else {
        std::cmp::min(1u64 << (retries - 1).min(5), 30)
    }
}

pub async fn wait_for_reconnect_delay_or_abort(
    state: &Arc<AppState>,
    attempt: u64,
    retries: u64,
) -> bool {
    let delay_secs = compute_reconnect_delay_secs(retries);
    if delay_secs == 0 {
        return is_current_connect_attempt(state, attempt).await;
    }

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(delay_secs);
    let poll_interval = std::time::Duration::from_millis(100);
    loop {
        if !is_current_connect_attempt(state, attempt).await {
            return false;
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            return true;
        }

        let remaining = deadline.saturating_duration_since(now);
        tokio::time::sleep(std::cmp::min(remaining, poll_interval)).await;
    }
}

fn collect_lookup_forwarder_ids(
    mode: Option<&rt_protocol::ReceiverMode>,
    subs: &[Subscription],
) -> Vec<String> {
    let from_mode = match mode {
        Some(rt_protocol::ReceiverMode::Live { streams, .. }) if !streams.is_empty() => Some(
            streams
                .iter()
                .map(|stream| stream.forwarder_id.clone())
                .collect::<Vec<_>>(),
        ),
        Some(rt_protocol::ReceiverMode::TargetedReplay { targets }) if !targets.is_empty() => Some(
            targets
                .iter()
                .map(|target| target.forwarder_id.clone())
                .collect::<Vec<_>>(),
        ),
        _ => None,
    };

    let mut forwarder_ids: Vec<String> = from_mode
        .unwrap_or_else(|| {
            subs.iter()
                .map(|sub| sub.forwarder_id.clone())
                .collect::<Vec<_>>()
        })
        .into_iter()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    forwarder_ids.sort();
    forwarder_ids
}

pub async fn refresh_chip_lookup(state: &Arc<AppState>) {
    let db = state.db.lock().await;
    let mode = db.load_receiver_mode().ok().flatten();
    let profile = db.load_profile().ok().flatten();
    let subs = db.load_subscriptions().unwrap_or_default();
    drop(db);

    let Some(p) = profile else {
        *state.chip_lookup.write().await = HashMap::new();
        return;
    };

    let forwarder_ids = match mode.as_ref() {
        Some(rt_protocol::ReceiverMode::Race { race_id }) => {
            match crate::control_api::fetch_forwarder_ids_for_race(
                &state.http_client,
                &p.server_url,
                &p.token,
                race_id,
            )
            .await
            {
                Ok(ids) => ids,
                Err(e) => {
                    warn!(error = %e, "failed to fetch race forwarder mappings");
                    return;
                }
            }
        }
        _ => collect_lookup_forwarder_ids(mode.as_ref(), &subs),
    };

    let result = if let Some(rt_protocol::ReceiverMode::Race { race_id }) = mode {
        crate::control_api::fetch_chip_lookup_for_race(
            &state.http_client,
            &p.server_url,
            &p.token,
            &race_id,
            &forwarder_ids,
        )
        .await
    } else {
        crate::control_api::fetch_chip_lookup_for_forwarders(
            &state.http_client,
            &p.server_url,
            &p.token,
            &forwarder_ids,
        )
        .await
    };

    let lookup = match result {
        Ok(lookup) => lookup,
        Err(e) => {
            warn!(error = %e, "failed to fetch participant lookup");
            return;
        }
    };

    let total_chips: usize = lookup.values().map(|m| m.len()).sum();
    if total_chips > 0 {
        state.logger.log(format!(
            "Loaded chip->participant mappings for {} forwarder(s) ({total_chips} chips)",
            lookup.len()
        ));
    }

    *state.chip_lookup.write().await = lookup;
}

fn should_refresh_stream_snapshot_for_dashboard_event(event_name: &str) -> bool {
    matches!(event_name, "stream_created" | "stream_updated" | "resync")
}

fn should_refresh_chip_lookup_for_dashboard_event(event_name: &str) -> bool {
    matches!(event_name, "forwarder_race_assigned")
}

fn should_emit_receiver_resync_for_dashboard_event(event_name: &str) -> bool {
    matches!(event_name, "resync")
}

async fn refresh_dashboard_snapshot(state: &Arc<AppState>) {
    state.emit_streams_snapshot().await;
}

#[derive(Default)]
struct PendingSseEvent {
    event_name: Option<String>,
    data_lines: Vec<String>,
}

fn parse_forwarder_metrics_dashboard_event(
    event_name: &str,
    data: &str,
) -> Option<crate::ui_events::ForwarderMetricsUpdate> {
    if event_name != "forwarder_metrics_updated" {
        return None;
    }
    serde_json::from_str(data).ok()
}

fn consume_sse_line_for_event(
    line: &str,
    pending_event: &mut PendingSseEvent,
) -> Option<(String, String)> {
    if line.is_empty() {
        let data = pending_event.data_lines.join("\n");
        pending_event.data_lines.clear();
        return pending_event
            .event_name
            .take()
            .map(|event_name| (event_name, data));
    }
    if line.starts_with(':') {
        return None;
    }
    if let Some(rest) = line.strip_prefix("event:") {
        let event_name = rest.trim();
        if event_name.is_empty() {
            pending_event.event_name = None;
            pending_event.data_lines.clear();
        } else {
            pending_event.event_name = Some(event_name.to_owned());
            pending_event.data_lines.clear();
        }
    } else if let Some(rest) = line.strip_prefix("data:") {
        pending_event.data_lines.push(rest.trim_start().to_owned());
    }
    None
}

use crate::control_api::http_base_url;

async fn open_upstream_dashboard_events(
    client: &reqwest::Client,
    events_url: &str,
    token: &str,
) -> Result<reqwest::Response, String> {
    let response = client
        .get(events_url)
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, "text/event-stream")
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("upstream returned {}", response.status()));
    }
    Ok(response)
}

async fn run_upstream_dashboard_sse_refresher(state: Arc<AppState>) {
    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            state.logger.log_at(
                UiLogLevel::Error,
                format!("failed to create upstream SSE client: {e}"),
            );
            return;
        }
    };

    loop {
        if *state.connection_state.borrow() != ConnectionState::Connected {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        }

        let profile = {
            let db = state.db.lock().await;
            match db.load_profile() {
                Ok(Some(p)) => Some(p),
                Ok(None) => None,
                Err(e) => {
                    tracing::warn!(error = %e, "failed to load profile for dashboard SSE");
                    None
                }
            }
        };
        let Some(profile) = profile else {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            continue;
        };

        let Some(base_url) = http_base_url(&profile.server_url) else {
            state.logger.log_at(
                UiLogLevel::Warn,
                format!(
                    "cannot derive upstream HTTP URL from profile server_url: {}",
                    profile.server_url
                ),
            );
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        };

        let events_url = format!("{base_url}/api/v1/events");

        let response =
            match open_upstream_dashboard_events(&client, &events_url, &profile.token).await {
                Ok(response) => response,
                Err(e) => {
                    if *state.connection_state.borrow() == ConnectionState::Connected {
                        state.logger.log_at(
                            UiLogLevel::Warn,
                            format!("upstream dashboard SSE refresh disconnected: {e}"),
                        );
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };

        match consume_upstream_dashboard_events(&state, response).await {
            Ok(()) => {}
            Err(e) => {
                if *state.connection_state.borrow() == ConnectionState::Connected {
                    state.logger.log_at(
                        UiLogLevel::Warn,
                        format!("upstream dashboard SSE refresh disconnected: {e}"),
                    );
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    }
}

async fn consume_upstream_dashboard_events(
    state: &Arc<AppState>,
    response: reqwest::Response,
) -> Result<(), String> {
    let mut response = response;
    let mut pending_line_bytes: Vec<u8> = Vec::new();
    let mut pending_event = PendingSseEvent::default();

    loop {
        if *state.connection_state.borrow() != ConnectionState::Connected {
            return Ok(());
        }

        let chunk = response
            .chunk()
            .await
            .map_err(|e| format!("read failed: {e}"))?;
        let Some(chunk) = chunk else {
            return Err("connection closed".to_owned());
        };
        pending_line_bytes.extend_from_slice(&chunk);

        while let Some(line_end_idx) = pending_line_bytes.iter().position(|byte| *byte == b'\n') {
            let mut line_bytes: Vec<u8> = pending_line_bytes.drain(..=line_end_idx).collect();
            if line_bytes.last().copied() == Some(b'\n') {
                line_bytes.pop();
            }
            if line_bytes.last().copied() == Some(b'\r') {
                line_bytes.pop();
            }

            let line = String::from_utf8_lossy(&line_bytes).into_owned();
            if let Some((event_name, data)) = consume_sse_line_for_event(&line, &mut pending_event)
            {
                if let Some(metrics) = parse_forwarder_metrics_dashboard_event(&event_name, &data) {
                    let _ = state.ui_tx.send(
                        crate::ui_events::ReceiverUiEvent::ForwarderMetricsUpdated(metrics),
                    );
                }
                if should_refresh_stream_snapshot_for_dashboard_event(&event_name) {
                    refresh_dashboard_snapshot(state).await;
                }
                if should_refresh_chip_lookup_for_dashboard_event(&event_name) {
                    refresh_chip_lookup(state).await;
                }
                if should_emit_receiver_resync_for_dashboard_event(&event_name) {
                    state.emit_resync();
                }
            }
        }
    }
}

pub async fn request_reconnect_if_connected(state: &Arc<AppState>) -> bool {
    state.request_reconnect_if_connected().await
}

pub async fn retry_connect_if_attempt_current(state: &Arc<AppState>, attempt: u64) -> bool {
    if !is_current_connect_attempt(state, attempt).await {
        state.logger.log("Ignoring stale connect retry request");
        return false;
    }
    state.request_retry_connect().await;
    true
}

pub async fn set_disconnected_if_attempt_current(state: &Arc<AppState>, attempt: u64) -> bool {
    if !is_current_connect_attempt(state, attempt).await {
        state.logger.log("Ignoring stale connect attempt result");
        return false;
    }
    state
        .set_connection_state(ConnectionState::Disconnected)
        .await;
    true
}

#[allow(clippy::type_complexity)]
pub async fn do_handshake<S>(
    mut ws: S,
    db: &Arc<tokio::sync::Mutex<Db>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ReceiverUiEvent>,
    receiver_id: &str,
    http_client: &reqwest::Client,
) -> (Result<String, crate::session::SessionError>, Option<S>)
where
    S: futures_util::Stream<
            Item = Result<
                tokio_tungstenite::tungstenite::protocol::Message,
                tokio_tungstenite::tungstenite::Error,
            >,
        > + futures_util::Sink<
            tokio_tungstenite::tungstenite::protocol::Message,
            Error = tokio_tungstenite::tungstenite::Error,
        > + Unpin,
{
    use futures_util::{SinkExt, StreamExt};
    use rt_protocol::{
        EarliestEpochOverride, ReceiverHelloV12, ReceiverMode, StreamRef, WsMessage,
    };
    use tokio_tungstenite::tungstenite::protocol::Message;

    let (resume, mut mode, earliest_epochs_rows, profile_url) = {
        let db = db.lock().await;
        let resume = match db.load_resume_cursors() {
            Ok(cursors) => cursors,
            Err(e) => return (Err(crate::session::SessionError::Db(e)), None),
        };
        let mode = match db.load_receiver_mode() {
            Ok(Some(mode)) => mode,
            Ok(None) => {
                let streams = match db.load_subscriptions() {
                    Ok(subs) => subs
                        .into_iter()
                        .map(|s| StreamRef {
                            forwarder_id: s.forwarder_id,
                            reader_ip: s.reader_ip,
                        })
                        .collect(),
                    Err(e) => return (Err(crate::session::SessionError::Db(e)), None),
                };
                ReceiverMode::Live {
                    streams,
                    earliest_epochs: vec![],
                }
            }
            Err(e) => return (Err(crate::session::SessionError::Db(e)), None),
        };
        let earliest_epochs_rows = match db.load_earliest_epochs() {
            Ok(rows) => rows,
            Err(e) => return (Err(crate::session::SessionError::Db(e)), None),
        };
        let profile_url = db.load_profile().ok().flatten().map(|p| p.server_url);
        let mode = if let ReceiverMode::Live { ref streams, .. } = mode {
            if streams.is_empty() {
                match db.load_subscriptions() {
                    Ok(subs) => {
                        let streams = subs
                            .into_iter()
                            .map(|s| StreamRef {
                                forwarder_id: s.forwarder_id,
                                reader_ip: s.reader_ip,
                            })
                            .collect();
                        if let ReceiverMode::Live {
                            earliest_epochs, ..
                        } = mode
                        {
                            ReceiverMode::Live {
                                streams,
                                earliest_epochs,
                            }
                        } else {
                            mode
                        }
                    }
                    Err(e) => return (Err(crate::session::SessionError::Db(e)), None),
                }
            } else {
                mode
            }
        } else {
            mode
        };
        (resume, mode, earliest_epochs_rows, profile_url)
    };

    let clear_targeted_mode_after_handshake = matches!(
        &mode,
        ReceiverMode::TargetedReplay { targets } if !targets.is_empty()
    );

    if let ReceiverMode::Live {
        ref mut streams,
        ref mut earliest_epochs,
    } = mode
    {
        let mut map: HashMap<(String, String), i64> = earliest_epochs_rows
            .into_iter()
            .map(|(fwd, ip, epoch)| ((fwd, ip), epoch))
            .collect();

        if let Some(url) = profile_url
            && let Ok(server_streams) =
                crate::control_api::fetch_server_streams(http_client, &url).await
        {
            let server_epoch_by_stream: HashMap<(String, String), i64> = server_streams
                .into_iter()
                .map(|stream| ((stream.forwarder_id, stream.reader_ip), stream.stream_epoch))
                .collect();

            for stream in streams.iter() {
                map.entry((stream.forwarder_id.clone(), stream.reader_ip.clone()))
                    .or_insert_with(|| {
                        server_epoch_by_stream
                            .get(&(stream.forwarder_id.clone(), stream.reader_ip.clone()))
                            .copied()
                            .unwrap_or(0)
                    });
            }
        }

        *earliest_epochs = map
            .into_iter()
            .map(
                |((forwarder_id, reader_ip), earliest_epoch)| EarliestEpochOverride {
                    forwarder_id,
                    reader_ip,
                    earliest_epoch,
                },
            )
            .collect();
        earliest_epochs.sort_by(|a, b| {
            a.forwarder_id
                .cmp(&b.forwarder_id)
                .then(a.reader_ip.cmp(&b.reader_ip))
        });
    }

    let hello = WsMessage::ReceiverHelloV12(ReceiverHelloV12 {
        receiver_id: receiver_id.to_owned(),
        mode,
        resume,
    });

    let hello_text = match serde_json::to_string(&hello) {
        Ok(t) => t,
        Err(e) => return (Err(crate::session::SessionError::Json(e)), None),
    };

    if let Err(e) = ws.send(Message::Text(hello_text.into())).await {
        return (Err(crate::session::SessionError::Ws(e)), None);
    }

    loop {
        let msg = match ws.next().await {
            None => return (Err(crate::session::SessionError::ConnectionClosed), None),
            Some(Err(e)) => return (Err(crate::session::SessionError::Ws(e)), None),
            Some(Ok(m)) => m,
        };

        let text = match msg {
            Message::Text(t) => t,
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => {
                return (Err(crate::session::SessionError::ConnectionClosed), None);
            }
            other => {
                error!(msg = ?other, "handshake: unexpected message type");
                return (
                    Err(crate::session::SessionError::UnexpectedFirstMessage),
                    None,
                );
            }
        };

        match serde_json::from_str::<WsMessage>(&text) {
            Ok(WsMessage::Heartbeat(hb)) => {
                if clear_targeted_mode_after_handshake {
                    let clear_mode = ReceiverMode::TargetedReplay {
                        targets: Vec::new(),
                    };
                    let db_guard = db.lock().await;
                    if let Err(e) = db_guard.save_receiver_mode(&clear_mode) {
                        return (Err(crate::session::SessionError::Db(e)), None);
                    }
                    drop(db_guard);
                }
                info!(session_id = %hb.session_id, "handshake complete");
                return (Ok(hb.session_id), Some(ws));
            }
            Ok(WsMessage::ReceiverModeApplied(applied)) => {
                info!(mode = %applied.mode_summary, streams = applied.resolved_stream_count, "mode applied before heartbeat");
                let _ = ui_tx.send(crate::ui_events::ReceiverUiEvent::LogEntry {
                    entry: format!(
                        "server applied mode: {} (resolved streams: {})",
                        applied.mode_summary, applied.resolved_stream_count
                    ),
                });
                for warning in applied.warnings {
                    warn!(warning = %warning, "server mode warning");
                    let _ = ui_tx.send(crate::ui_events::ReceiverUiEvent::LogEntry {
                        entry: format!("server mode warning: {warning}"),
                    });
                }
            }
            Ok(WsMessage::Error(err)) => {
                return (
                    Err(crate::session::SessionError::ServerError(format!(
                        "{}: {}",
                        err.code, err.message
                    ))),
                    None,
                );
            }
            Ok(other) => {
                error!("handshake: unexpected WsMessage variant: {other:?}");
                return (
                    Err(crate::session::SessionError::UnexpectedFirstMessage),
                    None,
                );
            }
            Err(e) => return (Err(crate::session::SessionError::Json(e)), None),
        }
    }
}

fn make_broadcast_sender(bus: &EventBus) -> tokio::sync::broadcast::Sender<rt_protocol::ReadEvent> {
    let (tx, mut rx) = tokio::sync::broadcast::channel::<rt_protocol::ReadEvent>(256);
    let bus = bus.clone();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => bus.publish(event),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    warn!(n, "relay lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
    tx
}

async fn cancel_session(
    task: &mut Option<tokio::task::JoinHandle<()>>,
    cancel_tx: &mut Option<watch::Sender<bool>>,
    logger: &rt_ui_log::UiLogger<crate::ReceiverUiEvent>,
) {
    if let Some(tx) = cancel_tx.take() {
        let _ = tx.send(true);
    }
    if let Some(handle) = task.take() {
        tokio::pin!(handle);
        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut handle).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                logger.log_at(UiLogLevel::Warn, format!("session task panicked: {e}"));
            }
            Err(_) => {
                logger.log_at(
                    UiLogLevel::Warn,
                    "session task did not exit in 5s; aborting",
                );
                handle.abort();
            }
        }
    }
}

pub async fn reconcile_proxies(
    subs: &[Subscription],
    proxies: &mut HashMap<String, LocalProxy>,
    event_bus: &EventBus,
    logger: &rt_ui_log::UiLogger<crate::ReceiverUiEvent>,
) {
    let assignments = resolve_ports(subs);

    let desired_ports: HashMap<String, u16> = assignments
        .iter()
        .filter_map(|(k, v)| {
            if let PortAssignment::Assigned(port) = v {
                Some((k.clone(), *port))
            } else {
                None
            }
        })
        .collect();

    proxies.retain(|key, proxy| match desired_ports.get(key) {
        Some(desired_port) if *desired_port == proxy.port => true,
        Some(desired_port) => {
            info!(
                key = %key,
                old_port = proxy.port,
                new_port = *desired_port,
                "restarting local proxy for port change"
            );
            proxy.shutdown();
            false
        }
        None => {
            info!(key = %key, port = proxy.port, "stopping removed local proxy");
            proxy.shutdown();
            false
        }
    });

    for sub in subs {
        let key = stream_key(&sub.forwarder_id, &sub.reader_ip);
        if proxies.contains_key(&key) {
            continue;
        }
        let port = match assignments.get(&key) {
            Some(PortAssignment::Assigned(p)) => *p,
            Some(PortAssignment::Collision {
                wanted,
                collides_with,
            }) => {
                logger.log_at(
                    UiLogLevel::Warn,
                    format!(
                        "port collision for {} (port {} used by {}) -- skipping",
                        key, wanted, collides_with,
                    ),
                );
                continue;
            }
            None => continue,
        };

        let stream_key_obj =
            crate::cache::StreamKey::new(sub.forwarder_id.clone(), sub.reader_ip.clone());
        let sender = event_bus.sender_for(&stream_key_obj);

        match LocalProxy::bind(port, sender).await {
            Ok(proxy) => {
                info!(key = %key, port = port, "local proxy started");
                proxies.insert(key, proxy);
            }
            Err(e) => {
                logger.log_at(
                    UiLogLevel::Error,
                    format!(
                        "failed to bind local proxy for {} on port {}: {}",
                        key, port, e
                    ),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::EventBus;
    use crate::control_api::AppState;
    use crate::db::Db;
    use crate::local_proxy::LocalProxy;
    use crate::ports::stream_key;
    use crate::ui_events::ReceiverUiEvent;
    use futures_util::{SinkExt, StreamExt};
    use rt_protocol::{Heartbeat, ReceiverMode, ReceiverModeApplied, ReplayTarget, WsMessage};
    use std::collections::HashMap;
    use std::future::Future;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::protocol::Message;

    async fn free_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        port
    }

    async fn run_raw_ws_server_once<H, Fut>(handler: H) -> (std::net::SocketAddr, JoinHandle<()>)
    where
        H: FnOnce(tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>) -> Fut
            + Send
            + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let ws = accept_async(stream).await.expect("ws accept");
            handler(ws).await;
        });
        (addr, task)
    }

    #[tokio::test]
    async fn reconcile_proxies_rebinds_when_port_changes() {
        let db = Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db, "test-receiver".to_owned());
        let event_bus = EventBus::new();
        let mut proxies: HashMap<String, LocalProxy> = HashMap::new();
        let first_port = free_port().await;
        let second_port = free_port().await;
        assert_ne!(first_port, second_port);

        let initial = vec![Subscription {
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            local_port_override: Some(first_port),
            event_type: crate::db::EventType::Finish,
        }];
        reconcile_proxies(&initial, &mut proxies, &event_bus, &state.logger).await;

        let key = stream_key("f1", "10.0.0.1:10000");
        assert_eq!(proxies.get(&key).expect("proxy").port, first_port);

        let updated = vec![Subscription {
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            local_port_override: Some(second_port),
            event_type: crate::db::EventType::Finish,
        }];
        reconcile_proxies(&updated, &mut proxies, &event_bus, &state.logger).await;

        assert_eq!(proxies.get(&key).expect("proxy").port, second_port);

        for proxy in proxies.values() {
            proxy.shutdown();
        }
    }

    #[tokio::test]
    async fn handshake_emits_mode_applied_warnings_to_ui_before_heartbeat() {
        let (addr, task) = run_raw_ws_server_once(|mut ws| async move {
            let _hello = ws.next().await.expect("hello frame").expect("hello ws");

            let mode_applied = WsMessage::ReceiverModeApplied(ReceiverModeApplied {
                mode_summary: "race=race-42".to_owned(),
                resolved_stream_count: 3,
                warnings: vec![
                    "stream fwd-1/10.0.0.1 unavailable".to_owned(),
                    "replay capped at 1000 events".to_owned(),
                ],
            });
            ws.send(Message::Text(
                serde_json::to_string(&mode_applied)
                    .expect("serialize mode")
                    .into(),
            ))
            .await
            .expect("send mode");

            let heartbeat = WsMessage::Heartbeat(Heartbeat {
                session_id: "session-handshake".to_owned(),
                device_id: "test-receiver".to_owned(),
            });
            ws.send(Message::Text(
                serde_json::to_string(&heartbeat)
                    .expect("serialize heartbeat")
                    .into(),
            ))
            .await
            .expect("send heartbeat");
        })
        .await;

        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("connect");
        let db = Arc::new(tokio::sync::Mutex::new(Db::open_in_memory().expect("db")));
        let (ui_tx, mut ui_rx) = tokio::sync::broadcast::channel::<ReceiverUiEvent>(8);

        let http_client = reqwest::Client::new();
        let (result, _ws) = do_handshake(ws, &db, &ui_tx, "test-receiver", &http_client).await;
        assert!(result.is_ok(), "handshake should succeed");

        let mut log_entries = Vec::new();
        while let Ok(event) = ui_rx.try_recv() {
            if let ReceiverUiEvent::LogEntry { entry } = event {
                log_entries.push(entry);
            }
        }

        assert!(
            log_entries
                .iter()
                .any(|entry| entry.contains("race=race-42") && entry.contains("3")),
            "expected mode summary log entry, got: {log_entries:?}"
        );
        assert!(
            log_entries
                .iter()
                .any(|entry| entry.contains("stream fwd-1/10.0.0.1 unavailable")),
            "expected first warning log entry, got: {log_entries:?}"
        );
        assert!(
            log_entries
                .iter()
                .any(|entry| entry.contains("replay capped at 1000 events")),
            "expected second warning log entry, got: {log_entries:?}"
        );

        task.await.expect("server task");
    }

    #[tokio::test]
    async fn handshake_sends_targeted_replay_targets_then_clears_them() {
        let (addr, task) = run_raw_ws_server_once(|mut ws| async move {
            let hello = ws.next().await.expect("hello frame").expect("hello ws");
            let Message::Text(hello_text) = hello else {
                panic!("expected text hello frame");
            };
            let hello_msg: WsMessage = serde_json::from_str(&hello_text).expect("parse hello");
            let WsMessage::ReceiverHelloV12(hello) = hello_msg else {
                panic!("expected receiver_hello_v12");
            };
            let ReceiverMode::TargetedReplay { targets } = hello.mode else {
                panic!("expected targeted replay mode");
            };
            assert_eq!(hello.receiver_id, "test-receiver");
            assert_eq!(targets.len(), 1);
            assert_eq!(targets[0].forwarder_id, "f1");
            assert_eq!(targets[0].reader_ip, "10.0.0.1");
            assert_eq!(targets[0].stream_epoch, 4);

            let heartbeat = WsMessage::Heartbeat(Heartbeat {
                session_id: "session-targeted".to_owned(),
                device_id: "test-receiver".to_owned(),
            });
            ws.send(Message::Text(
                serde_json::to_string(&heartbeat)
                    .expect("serialize heartbeat")
                    .into(),
            ))
            .await
            .expect("send heartbeat");
        })
        .await;

        let (ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("connect");
        let mut db = Db::open_in_memory().expect("db");
        db.save_profile("ws://server.example", "token", "check-and-download", None)
            .expect("save profile");
        let initial_mode = ReceiverMode::TargetedReplay {
            targets: vec![ReplayTarget {
                forwarder_id: "f1".to_owned(),
                reader_ip: "10.0.0.1".to_owned(),
                stream_epoch: 4,
                from_seq: 1,
            }],
        };
        db.save_receiver_mode(&initial_mode).expect("save mode");
        assert_eq!(
            db.load_receiver_mode().expect("load mode before handshake"),
            Some(initial_mode)
        );

        let db = Arc::new(tokio::sync::Mutex::new(db));
        let (ui_tx, _ui_rx) = tokio::sync::broadcast::channel::<ReceiverUiEvent>(8);
        let http_client = reqwest::Client::new();
        let (result, _ws) = do_handshake(ws, &db, &ui_tx, "test-receiver", &http_client).await;
        assert!(result.is_ok(), "handshake should succeed");
        assert_eq!(
            db.lock()
                .await
                .load_receiver_mode()
                .expect("load mode after handshake"),
            Some(ReceiverMode::TargetedReplay {
                targets: Vec::new()
            })
        );

        task.await.expect("server task");
    }

    #[tokio::test]
    async fn dropped_session_does_not_reconnect_when_disconnect_in_progress() {
        let db = Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db, "test-receiver".to_owned());

        state.request_connect().await;
        state
            .set_connection_state(ConnectionState::Disconnecting)
            .await;
        let before_attempt = state.current_connect_attempt();

        let reissued = request_reconnect_if_connected(&state).await;

        assert!(!reissued);
        assert_eq!(state.current_connect_attempt(), before_attempt);
        assert_eq!(
            *state.connection_state.borrow(),
            ConnectionState::Disconnecting
        );
    }

    #[tokio::test]
    async fn recoverable_failure_reissues_connect_for_current_attempt() {
        let db = Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db, "test-receiver".to_owned());

        state.request_connect().await;
        let attempt = state.current_connect_attempt();

        let retried = retry_connect_if_attempt_current(&state, attempt).await;

        assert!(retried);
        assert!(state.current_connect_attempt() > attempt);
        assert_eq!(
            *state.connection_state.borrow(),
            ConnectionState::Connecting
        );
    }

    #[tokio::test]
    async fn recoverable_failure_does_not_reissue_when_attempt_is_stale() {
        let db = Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db, "test-receiver".to_owned());

        state.request_connect().await;
        let stale_attempt = state.current_connect_attempt();
        state.request_connect().await;

        let retried = retry_connect_if_attempt_current(&state, stale_attempt).await;

        assert!(!retried);
    }

    #[tokio::test]
    async fn watch_connection_state_observes_current_connecting_state() {
        let db = Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db, "test-receiver".to_owned());

        state.request_connect().await;

        let mut conn_state_rx = state.conn_rx();
        let observed = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            watch_connection_state(&mut conn_state_rx),
        )
        .await;

        assert_eq!(
            observed.expect("watcher should not miss the current connecting state"),
            ConnectionState::Connecting
        );
    }

    #[test]
    fn collect_lookup_forwarder_ids_prefers_live_mode_streams_over_subscriptions() {
        let subs = vec![Subscription {
            forwarder_id: "sub-fwd".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
        }];
        let mode = ReceiverMode::Live {
            streams: vec![
                rt_protocol::StreamRef {
                    forwarder_id: "mode-fwd-a".to_owned(),
                    reader_ip: "10.0.0.2:10000".to_owned(),
                },
                rt_protocol::StreamRef {
                    forwarder_id: "mode-fwd-b".to_owned(),
                    reader_ip: "10.0.0.3:10000".to_owned(),
                },
                rt_protocol::StreamRef {
                    forwarder_id: "mode-fwd-a".to_owned(),
                    reader_ip: "10.0.0.4:10000".to_owned(),
                },
            ],
            earliest_epochs: Vec::new(),
        };

        let forwarder_ids = collect_lookup_forwarder_ids(Some(&mode), &subs);

        assert_eq!(forwarder_ids, vec!["mode-fwd-a", "mode-fwd-b"]);
    }

    #[test]
    fn collect_lookup_forwarder_ids_uses_targeted_replay_targets() {
        let subs = vec![Subscription {
            forwarder_id: "sub-fwd".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
        }];
        let mode = ReceiverMode::TargetedReplay {
            targets: vec![
                ReplayTarget {
                    forwarder_id: "target-fwd".to_owned(),
                    reader_ip: "10.0.0.2:10000".to_owned(),
                    stream_epoch: 4,
                    from_seq: 1,
                },
                ReplayTarget {
                    forwarder_id: "target-fwd".to_owned(),
                    reader_ip: "10.0.0.3:10000".to_owned(),
                    stream_epoch: 5,
                    from_seq: 1,
                },
            ],
        };

        let forwarder_ids = collect_lookup_forwarder_ids(Some(&mode), &subs);

        assert_eq!(forwarder_ids, vec!["target-fwd"]);
    }

    #[test]
    fn reconnect_backoff_caps_at_thirty_seconds() {
        assert_eq!(compute_reconnect_delay_secs(0), 0);
        assert_eq!(compute_reconnect_delay_secs(1), 1);
        assert_eq!(compute_reconnect_delay_secs(2), 2);
        assert_eq!(compute_reconnect_delay_secs(3), 4);
        assert_eq!(compute_reconnect_delay_secs(4), 8);
        assert_eq!(compute_reconnect_delay_secs(5), 16);
        assert_eq!(compute_reconnect_delay_secs(6), 30);
        assert_eq!(compute_reconnect_delay_secs(10), 30);
    }

    #[test]
    fn shutdown_signal_exit_helper_only_exits_for_terminate() {
        assert!(!should_exit_on_shutdown_signal(&ShutdownSignal::Disconnect));
        assert!(should_exit_on_shutdown_signal(&ShutdownSignal::Terminate));
    }

    #[tokio::test]
    async fn reconnect_backoff_wait_aborts_when_state_changes() {
        let db = Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db, "test-receiver".to_owned());

        state.request_connect().await;
        let attempt = state.current_connect_attempt();

        let state_for_wait = Arc::clone(&state);
        let wait_handle = tokio::spawn(async move {
            wait_for_reconnect_delay_or_abort(&state_for_wait, attempt, 6).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        state
            .set_connection_state(ConnectionState::Disconnecting)
            .await;

        let completed = tokio::time::timeout(std::time::Duration::from_millis(250), wait_handle)
            .await
            .expect("wait helper should finish quickly")
            .expect("wait task should not panic");
        assert!(!completed);
    }

    #[tokio::test(start_paused = true)]
    async fn reconnect_backoff_wait_completes_when_attempt_stays_current() {
        let db = Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db, "test-receiver".to_owned());

        state.request_connect().await;
        let attempt = state.current_connect_attempt();

        let state_for_wait = Arc::clone(&state);
        let wait_handle = tokio::spawn(async move {
            wait_for_reconnect_delay_or_abort(&state_for_wait, attempt, 1).await
        });

        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(1)).await;

        let completed = wait_handle.await.expect("wait task should not panic");
        assert!(completed);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_dbf_writer_aborts_stuck_task_after_timeout() {
        struct DropFlag(std::sync::Arc<std::sync::atomic::AtomicBool>);

        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let (cancel_tx, _cancel_rx) = watch::channel(false);
        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let dropped_for_task = std::sync::Arc::clone(&dropped);

        let stuck_task = tokio::spawn(async move {
            let _guard = DropFlag(dropped_for_task);
            std::future::pending::<()>().await;
        });

        let stop_task = tokio::spawn(async move {
            stop_dbf_writer(Some(cancel_tx), None, Some(stuck_task)).await;
        });

        tokio::task::yield_now().await;
        tokio::time::advance(std::time::Duration::from_secs(2)).await;

        stop_task.await.expect("stop task should not panic");
        assert!(
            dropped.load(std::sync::atomic::Ordering::SeqCst),
            "timed out DBF writer task should be aborted so its future is dropped"
        );
    }

    #[tokio::test]
    async fn stop_dbf_writer_does_not_allow_blocked_write_to_complete_after_return() {
        use fs2::FileExt;
        use tokio::sync::broadcast;

        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("test.db");
        let dbf_path = dir.path().join("test.dbf");
        let db = Db::open(&db_path).unwrap();
        db.save_subscription("f1", "10.0.0.1", None, None).unwrap();
        crate::dbf_writer::create_empty_dbf(&dbf_path).unwrap();

        let db = Arc::new(tokio::sync::Mutex::new(db));
        let (tx, _) = broadcast::channel::<rt_protocol::ReadEvent>(16);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let rx = tx.subscribe();
        let (ui_tx, _) = broadcast::channel(16);
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let lock_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&dbf_path)
            .unwrap();
        lock_file.lock_exclusive().unwrap();

        let dbf_path_string = dbf_path.to_str().unwrap().to_owned();
        let writer_task = tokio::spawn({
            let db = Arc::clone(&db);
            let cancel_flag = Arc::clone(&cancel_flag);
            async move {
                crate::dbf_writer::run_dbf_writer(
                    rx,
                    db,
                    cancel_rx,
                    cancel_flag,
                    dbf_path_string,
                    ui_tx,
                )
                .await;
            }
        });

        tx.send(rt_protocol::ReadEvent {
            forwarder_id: "f1".to_owned(),
            reader_ip: "10.0.0.1".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "T".to_owned(),
            raw_frame: b"aa400000000123450a2a01123018455927a7".to_vec(),
            read_type: "RAW".to_owned(),
        })
        .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        tokio::time::timeout(
            std::time::Duration::from_secs(3),
            stop_dbf_writer(Some(cancel_tx), Some(cancel_flag), Some(writer_task)),
        )
        .await
        .expect("DBF writer stop should return after timing out");

        lock_file.unlock().unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;

        let mut reader = dbase::Reader::from_path(&dbf_path).unwrap();
        let records: Vec<dbase::Record> = reader.read().unwrap();
        assert_eq!(
            records.len(),
            0,
            "no stale DBF write should land after stop_dbf_writer returns"
        );
    }

    #[tokio::test]
    async fn reconnect_backoff_wait_aborts_when_attempt_becomes_stale() {
        let db = Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db, "test-receiver".to_owned());

        state.request_connect().await;
        let stale_attempt = state.current_connect_attempt();

        let state_for_wait = Arc::clone(&state);
        let wait_handle = tokio::spawn(async move {
            wait_for_reconnect_delay_or_abort(&state_for_wait, stale_attempt, 6).await
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        state.request_connect().await;

        let completed = tokio::time::timeout(std::time::Duration::from_millis(250), wait_handle)
            .await
            .expect("wait helper should finish quickly")
            .expect("wait task should not panic");
        assert!(!completed);
    }

    #[tokio::test]
    async fn manual_connect_resets_retry_backoff_state() {
        let db = Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db, "test-receiver".to_owned());

        state.request_connect().await;
        let attempt = state.current_connect_attempt();
        let retried = retry_connect_if_attempt_current(&state, attempt).await;
        assert!(retried);
        assert_eq!(state.current_retry_streak(), 1);
        assert_eq!(
            compute_reconnect_delay_secs(state.current_retry_streak()),
            1
        );

        state.request_connect().await;
        assert_eq!(state.current_retry_streak(), 0);
        assert_eq!(
            compute_reconnect_delay_secs(state.current_retry_streak()),
            0
        );
    }

    #[test]
    fn resolve_receiver_id_prefers_cli() {
        let db = Db::open_in_memory().unwrap();
        db.save_receiver_id("recv-db").unwrap();
        let id = resolve_receiver_id(Some("recv-cli".to_owned()), &db).unwrap();
        assert_eq!(id, "recv-cli");
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.receiver_id, Some("recv-cli".to_owned()));
    }

    #[test]
    fn resolve_receiver_id_falls_back_to_db() {
        let db = Db::open_in_memory().unwrap();
        db.save_receiver_id("recv-db").unwrap();
        let id = resolve_receiver_id(None, &db).unwrap();
        assert_eq!(id, "recv-db");
    }

    #[test]
    fn resolve_receiver_id_auto_generates_when_db_empty() {
        let db = Db::open_in_memory().unwrap();
        let id = resolve_receiver_id(None, &db).unwrap();
        assert!(id.starts_with("recv-"), "expected recv- prefix, got: {id}");
        assert_eq!(id.len(), 13);
        let p = db.load_profile().unwrap().unwrap();
        assert_eq!(p.receiver_id, Some(id.clone()));
    }

    #[test]
    fn resolve_receiver_id_skips_whitespace_cli() {
        let db = Db::open_in_memory().unwrap();
        db.save_receiver_id("recv-db").unwrap();
        let id = resolve_receiver_id(Some("  ".to_owned()), &db).unwrap();
        assert_eq!(id, "recv-db");
    }

    #[test]
    fn generate_receiver_id_format() {
        let id = generate_receiver_id();
        assert!(id.starts_with("recv-"));
        assert_eq!(id.len(), 13);
        assert!(id[5..].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn profile_with_url_and_token_is_required_for_autoconnect() {
        assert!(!profile_has_connect_credentials(None));
        assert!(!profile_has_connect_credentials(Some(
            &crate::db::Profile {
                server_url: "ws://server".to_owned(),
                token: String::new(),
                update_mode: "check-only".to_owned(),
                receiver_id: None,
            }
        )));
        assert!(!profile_has_connect_credentials(Some(
            &crate::db::Profile {
                server_url: String::new(),
                token: "token".to_owned(),
                update_mode: "check-only".to_owned(),
                receiver_id: None,
            }
        )));
        assert!(profile_has_connect_credentials(Some(&crate::db::Profile {
            server_url: "ws://server".to_owned(),
            token: "token".to_owned(),
            update_mode: "check-only".to_owned(),
            receiver_id: None,
        })));
    }

    #[test]
    fn dashboard_event_filter_only_refreshes_on_stream_metadata_changes() {
        assert!(should_refresh_stream_snapshot_for_dashboard_event(
            "stream_created"
        ));
        assert!(should_refresh_stream_snapshot_for_dashboard_event(
            "stream_updated"
        ));
        assert!(should_refresh_stream_snapshot_for_dashboard_event("resync"));

        assert!(!should_refresh_stream_snapshot_for_dashboard_event(
            "metrics_updated"
        ));
        assert!(!should_refresh_stream_snapshot_for_dashboard_event(
            "forwarder_race_assigned"
        ));
        assert!(!should_refresh_stream_snapshot_for_dashboard_event(
            "log_entry"
        ));
        assert!(!should_refresh_stream_snapshot_for_dashboard_event(
            "unknown_event"
        ));
    }

    #[test]
    fn dashboard_event_filter_emits_receiver_resync_only_for_resync_events() {
        assert!(should_emit_receiver_resync_for_dashboard_event("resync"));

        assert!(!should_emit_receiver_resync_for_dashboard_event(
            "stream_created"
        ));
        assert!(!should_emit_receiver_resync_for_dashboard_event(
            "stream_updated"
        ));
        assert!(!should_emit_receiver_resync_for_dashboard_event(
            "metrics_updated"
        ));
        assert!(!should_emit_receiver_resync_for_dashboard_event(
            "forwarder_race_assigned"
        ));
        assert!(!should_emit_receiver_resync_for_dashboard_event(
            "log_entry"
        ));
        assert!(!should_emit_receiver_resync_for_dashboard_event(
            "unknown_event"
        ));
    }

    #[test]
    fn sse_event_parsing_emits_completed_event_name_on_frame_boundary() {
        let mut pending_event = PendingSseEvent::default();
        assert_eq!(
            consume_sse_line_for_event("event: stream_updated", &mut pending_event),
            None
        );
        assert_eq!(
            consume_sse_line_for_event("data: {\"type\":\"stream_updated\"}", &mut pending_event),
            None
        );
        assert_eq!(
            consume_sse_line_for_event("", &mut pending_event),
            Some((
                "stream_updated".to_owned(),
                "{\"type\":\"stream_updated\"}".to_owned()
            ))
        );
        assert!(pending_event.event_name.is_none());
        assert!(pending_event.data_lines.is_empty());
    }

    #[test]
    fn sse_event_parsing_ignores_comments_and_data_only_frames() {
        let mut pending_event = PendingSseEvent::default();
        assert_eq!(
            consume_sse_line_for_event(": keepalive", &mut pending_event),
            None
        );
        assert_eq!(
            consume_sse_line_for_event("data: keepalive", &mut pending_event),
            None
        );
        assert_eq!(consume_sse_line_for_event("", &mut pending_event), None);
    }

    #[test]
    fn sse_parser_captures_data_payload() {
        let mut pending_event = PendingSseEvent::default();

        assert_eq!(
            consume_sse_line_for_event("event: metrics_updated", &mut pending_event),
            None
        );
        assert_eq!(
            consume_sse_line_for_event(
                r#"data: {"stream_id":"abc","raw_count":10}"#,
                &mut pending_event
            ),
            None
        );
        let result = consume_sse_line_for_event("", &mut pending_event);
        assert_eq!(
            result,
            Some((
                "metrics_updated".to_owned(),
                r#"{"stream_id":"abc","raw_count":10}"#.to_owned()
            ))
        );
        assert!(pending_event.event_name.is_none());
        assert!(pending_event.data_lines.is_empty());
    }

    #[test]
    fn sse_parser_returns_empty_data_when_no_data_line() {
        let mut pending_event = PendingSseEvent::default();

        consume_sse_line_for_event("event: stream_created", &mut pending_event);
        let result = consume_sse_line_for_event("", &mut pending_event);
        assert_eq!(result, Some(("stream_created".to_owned(), "".to_owned())));
    }

    #[test]
    fn sse_parser_concatenates_multiline_data() {
        let mut pending_event = PendingSseEvent::default();
        assert!(consume_sse_line_for_event("event:test", &mut pending_event).is_none());
        assert!(consume_sse_line_for_event("data:line1", &mut pending_event).is_none());
        assert!(consume_sse_line_for_event("data:line2", &mut pending_event).is_none());
        let result = consume_sse_line_for_event("", &mut pending_event);
        assert_eq!(result, Some(("test".to_owned(), "line1\nline2".to_owned())));
    }

    #[test]
    fn parse_forwarder_metrics_dashboard_event_extracts_payload() {
        let parsed = parse_forwarder_metrics_dashboard_event(
            "forwarder_metrics_updated",
            r#"{"type":"forwarder_metrics_updated","forwarder_id":"fwd-1","unique_chips":4,"total_reads":15,"last_read_at":"2026-03-21T12:34:56.000Z"}"#,
        )
        .expect("forwarder metrics payload should parse");
        assert_eq!(parsed.forwarder_id, "fwd-1");
        assert_eq!(parsed.unique_chips, 4);
        assert_eq!(parsed.total_reads, 15);
        assert_eq!(
            parsed.last_read_at.as_deref(),
            Some("2026-03-21T12:34:56.000Z")
        );
    }

    #[test]
    fn dashboard_event_filter_does_not_trigger_stream_snapshot_for_forwarder_metrics() {
        assert!(!should_refresh_stream_snapshot_for_dashboard_event(
            "forwarder_metrics_updated"
        ));
        assert!(!should_emit_receiver_resync_for_dashboard_event(
            "forwarder_metrics_updated"
        ));
    }

    #[tokio::test]
    async fn stale_connect_attempt_failure_does_not_force_disconnected() {
        let db = Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db, "test-receiver".to_owned());

        state.request_connect().await;
        let stale_attempt = state.current_connect_attempt();
        state.request_connect().await;

        let transitioned = set_disconnected_if_attempt_current(&state, stale_attempt).await;
        assert!(!transitioned);
        assert_eq!(
            *state.connection_state.borrow(),
            ConnectionState::Connecting
        );
    }
}
