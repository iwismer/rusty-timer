use receiver::Subscription;
use receiver::cache::EventBus;
use receiver::control_api::{AppState, ConnectionState};
use receiver::db::Db;
use receiver::local_proxy::LocalProxy;
use receiver::ports::{PortAssignment, resolve_ports, stream_key};
use rt_ui_log::UiLogLevel;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::watch;
use tokio_tungstenite::connect_async;
use tracing::{error, info, warn};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    // -------------------------------------------------------------------------
    // 1. Open SQLite DB
    // -------------------------------------------------------------------------
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("rusty-timer")
        .join("receiver");

    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        eprintln!("FATAL: could not create data directory: {e}");
        std::process::exit(1);
    }

    let db_path = data_dir.join("receiver.sqlite3");
    let db = Db::open(&db_path).unwrap_or_else(|e| {
        eprintln!("FATAL: failed to open DB: {e}");
        std::process::exit(1);
    });
    db.integrity_check().unwrap_or_else(|e| {
        eprintln!("FATAL: integrity_check failed: {e}");
        std::process::exit(1);
    });

    // -------------------------------------------------------------------------
    // 2. Create AppState
    // -------------------------------------------------------------------------
    let (state, mut shutdown_rx) = AppState::new(db);
    state.logger.log("Receiver started");

    // -------------------------------------------------------------------------
    // 4. Load profile and restore subscriptions
    // -------------------------------------------------------------------------
    let update_mode: rt_updater::UpdateMode;
    let has_profile: bool;
    {
        let db = state.db.lock().await;
        let profile = db.load_profile().ok().flatten();
        has_profile = profile_has_connect_credentials(profile.as_ref());
        if let Some(ref p) = profile {
            *state.upstream_url.write().await = Some(p.server_url.clone());
            info!(url = %p.server_url, "restored profile");
        }
        update_mode = profile
            .as_ref()
            .and_then(|p| {
                serde_json::from_value::<rt_updater::UpdateMode>(serde_json::Value::String(
                    p.update_mode.clone(),
                ))
                .ok()
            })
            .unwrap_or_default();
    }

    *state.update_mode.write().await = update_mode;

    let event_bus = EventBus::new();

    // Start local proxies for any saved subscriptions on startup.
    let initial_subs = {
        let db = state.db.lock().await;
        db.load_subscriptions().unwrap_or_default()
    };
    // Map from stream-key -> LocalProxy handle.
    let mut proxies: HashMap<String, LocalProxy> = HashMap::new();
    reconcile_proxies(&initial_subs, &mut proxies, &event_bus, &state.logger).await;

    // Auto-connect on startup if a profile with URL and token exists.
    if has_profile {
        state.logger.log("Auto-connecting to server");
        state.request_connect().await;
    }

    // -------------------------------------------------------------------------
    // 3. Start Axum control API on 127.0.0.1:9090
    // -------------------------------------------------------------------------
    let router = receiver::control_api::build_router(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:9090")
        .await
        .unwrap_or_else(|e| {
            eprintln!("FATAL: failed to bind control API on 127.0.0.1:9090: {e}");
            std::process::exit(1);
        });
    state.logger.log("Control API listening on 127.0.0.1:9090");

    let api_state = Arc::clone(&state);
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            error!(error = %e, "control API exited");
        }
        // If the control API exits unexpectedly, signal shutdown.
        let _ = api_state.shutdown_tx.send(true);
    });

    // Spawn background update check
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if update_mode == rt_updater::UpdateMode::Disabled {
                state.logger.log("auto-update disabled by configuration");
                return;
            }

            let checker = match rt_updater::UpdateChecker::new(
                "iwismer",
                "rusty-timer",
                "receiver",
                env!("CARGO_PKG_VERSION"),
            ) {
                Ok(c) => c,
                Err(e) => {
                    state.logger.log_at(
                        UiLogLevel::Warn,
                        format!("failed to create update checker: {e}"),
                    );
                    return;
                }
            };

            match checker.check().await {
                Ok(rt_updater::UpdateStatus::Available { ref version }) => {
                    state.logger.log(format!("Update v{version} available"));
                    *state.update_status.write().await = rt_updater::UpdateStatus::Available {
                        version: version.clone(),
                    };
                    let _ = state
                        .ui_tx
                        .send(receiver::ReceiverUiEvent::UpdateStatusChanged {
                            status: rt_updater::UpdateStatus::Available {
                                version: version.clone(),
                            },
                        });

                    if update_mode == rt_updater::UpdateMode::CheckAndDownload {
                        match checker.download(version).await {
                            Ok(path) => {
                                state
                                    .logger
                                    .log(format!("Update v{version} downloaded and staged"));
                                *state.update_status.write().await =
                                    rt_updater::UpdateStatus::Downloaded {
                                        version: version.clone(),
                                    };
                                *state.staged_update_path.write().await = Some(path);

                                let _ = state.ui_tx.send(
                                    receiver::ReceiverUiEvent::UpdateStatusChanged {
                                        status: rt_updater::UpdateStatus::Downloaded {
                                            version: version.clone(),
                                        },
                                    },
                                );
                            }
                            Err(e) => {
                                state.logger.log_at(
                                    UiLogLevel::Warn,
                                    format!("update download failed: {e}"),
                                );
                                *state.update_status.write().await =
                                    rt_updater::UpdateStatus::Failed {
                                        error: e.to_string(),
                                    };
                                let _ = state.ui_tx.send(
                                    receiver::ReceiverUiEvent::UpdateStatusChanged {
                                        status: rt_updater::UpdateStatus::Failed {
                                            error: e.to_string(),
                                        },
                                    },
                                );
                            }
                        }
                    }
                }
                Ok(_) => {
                    state.logger.log("receiver is up to date");
                }
                Err(e) => {
                    state
                        .logger
                        .log_at(UiLogLevel::Warn, format!("update check failed: {e}"));
                    *state.update_status.write().await = rt_updater::UpdateStatus::Failed {
                        error: e.to_string(),
                    };
                    let _ = state
                        .ui_tx
                        .send(receiver::ReceiverUiEvent::UpdateStatusChanged {
                            status: rt_updater::UpdateStatus::Failed {
                                error: e.to_string(),
                            },
                        });
                }
            }
        });
    }
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            run_upstream_dashboard_sse_refresher(state).await;
        });
    }

    // -------------------------------------------------------------------------
    // Event loop: watch connection_state + reconcile subscriptions
    // -------------------------------------------------------------------------
    // Optional cancel sender for the active WS session task; None when idle.
    let mut session_cancel_tx: Option<watch::Sender<bool>> = None;
    let mut session_task: Option<tokio::task::JoinHandle<()>> = None;

    // Interval for subscription reconciliation polling.
    let mut reconcile_interval = tokio::time::interval(std::time::Duration::from_millis(500));
    reconcile_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Track last-known subscriptions to detect changes.
    let mut last_subs: Vec<Subscription> = initial_subs;

    loop {
        tokio::select! {
            biased;

            // ------------------------------------------------------------------
            // Graceful shutdown: ctrl-c or SIGTERM
            // ------------------------------------------------------------------
            _ = tokio::signal::ctrl_c() => {
                info!("received ctrl-c, shutting down");
                break;
            }

            // ------------------------------------------------------------------
            // Shutdown signal from control API (e.g. disconnect sends true)
            // We only break the outer loop here if it's a process-level shutdown
            // request (not a disconnect-only signal).  The disconnect path
            // is handled by the connection_state watcher below.
            // ------------------------------------------------------------------
            result = shutdown_rx.changed() => {
                if result.is_err() {
                    // Sender dropped — exit.
                    info!("shutdown channel closed, exiting");
                    break;
                }
                // The shutdown_tx is also used by post_disconnect to cancel the
                // WS session; we don't terminate the process on every send.
                // We only terminate if the connection state is truly shutting
                // down the process (handled via ctrl-c above) — so this branch
                // intentionally does nothing extra here; the connection-state
                // watcher below takes care of Disconnecting.
            }

            // ------------------------------------------------------------------
            // Subscription reconciliation (polling every 500 ms)
            // ------------------------------------------------------------------
            _ = reconcile_interval.tick() => {
                let current_subs = {
                    let db = state.db.lock().await;
                    db.load_subscriptions().unwrap_or_default()
                };
                if current_subs != last_subs {
                    reconcile_proxies(&current_subs, &mut proxies, &event_bus, &state.logger).await;
                    let keep: HashSet<receiver::StreamKey> = current_subs
                        .iter()
                        .map(|s| receiver::StreamKey::new(&s.forwarder_id, &s.reader_ip))
                        .collect();
                    state.stream_counts.retain_keys(&keep);
                    state.logger.log(format!("Subscriptions changed ({} streams)", current_subs.len()));
                    state.emit_streams_snapshot().await;
                    last_subs = current_subs;
                }
            }

            // ------------------------------------------------------------------
            // Connection state changes
            // ------------------------------------------------------------------
            result = watch_connection_state(Arc::clone(&state)) => {
                match result {
                    ConnectionState::Connecting => {
                        let attempt = state.current_connect_attempt();

                        // Exponential backoff for automatic retries. Manual connect
                        // requests reset the retry streak and remain immediate.
                        let retries = state.current_retry_streak();
                        let delay_secs = compute_reconnect_delay_secs(retries);
                        if delay_secs > 0 {
                            state.logger.log(format!("Reconnecting in {delay_secs}s"));
                        }
                        if !wait_for_reconnect_delay_or_abort(&state, attempt, retries).await {
                            continue;
                        }

                        // Cancel any existing session first.
                        *state.session_command_tx.write().await = None;
                        cancel_session(&mut session_task, &mut session_cancel_tx, &state.logger).await;

                        let url_opt = state.upstream_url.read().await.clone();
                        match url_opt {
                            None => {
                                state.logger.log_at(UiLogLevel::Warn, "No upstream URL configured");
                                let _ =
                                    set_disconnected_if_attempt_current(&state, attempt).await;
                            }
                            Some(base_url) => {
                                // Build the full WS URL from the base URL.
                                let ws_url = format!(
                                    "{}/ws/v1.2/receivers",
                                    base_url.trim_end_matches('/')
                                );
                                // Read the token from the saved profile so we can
                                // authenticate the WebSocket upgrade request.
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
                                    receiver::build_authenticated_request(ws_url.as_str(), &token);
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
                                        // Perform the receiver hello / heartbeat handshake.
                                        let (session_result, ws) = {
                                            let db = state.db.lock().await;
                                            do_handshake(ws, &db, &state.ui_tx).await
                                        };
                                        match (session_result, ws) {
                                            (Err(e), _) => {
                                                state.logger.log_at(UiLogLevel::Error, format!("Handshake failed: {e}"));
                                                let _ = retry_connect_if_attempt_current(
                                                    &state, attempt,
                                                )
                                                .await;
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

                                                let (cancel_tx, cancel_rx) =
                                                    watch::channel(false);
                                                let (session_cmd_tx, session_cmd_rx) =
                                                    tokio::sync::mpsc::unbounded_channel();
                                                *state.session_command_tx.write().await =
                                                    Some(session_cmd_tx);
                                                let db_arc = Arc::clone(&state.db);
                                                let bus = event_bus.clone();
                                                let counts = state.stream_counts.clone();
                                                let ui_tx = state.ui_tx.clone();
                                                let paused_streams = Arc::clone(&state.paused_streams);
                                                let all_paused = Arc::clone(&state.all_paused);
                                                let st = Arc::clone(&state);
                                                let handle = tokio::spawn(async move {
                                                    let event_tx = make_broadcast_sender(&bus);
                                                    let deps = receiver::session::SessionLoopDeps {
                                                        db: db_arc,
                                                        event_tx,
                                                        stream_counts: counts,
                                                        ui_tx,
                                                        shutdown: cancel_rx,
                                                        paused_streams,
                                                        all_paused,
                                                        control_rx: Some(session_cmd_rx),
                                                    };
                                                    let result = receiver::session::run_session_loop(
                                                        ws, session_id, deps,
                                                    )
                                                    .await;
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
                                                    *st.session_command_tx.write().await = None;
                                                    if request_reconnect_if_connected(&st).await {
                                                        // Unexpected drop — auto-reconnect.
                                                        st.logger.log("Connection lost, will reconnect");
                                                        st.emit_streams_snapshot().await;
                                                    } else if *st.connection_state.read().await
                                                        == ConnectionState::Disconnecting
                                                    {
                                                        // User-initiated disconnect.
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
                        *state.session_command_tx.write().await = None;
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
        }
    }

    // -------------------------------------------------------------------------
    // 8. Graceful shutdown — close WS session and release TCP ports
    // -------------------------------------------------------------------------
    state.logger.log("shutdown signal received");
    *state.session_command_tx.write().await = None;
    cancel_session(&mut session_task, &mut session_cancel_tx, &state.logger).await;
    for (key, proxy) in proxies.drain() {
        info!(key = %key, port = proxy.port, "closing local proxy");
        proxy.shutdown();
    }
    info!("receiver stopped");
}

// ---------------------------------------------------------------------------
// Helper: watch connection_state and return the new value when it changes.
// ---------------------------------------------------------------------------
async fn watch_connection_state(state: Arc<AppState>) -> ConnectionState {
    // Poll until the state changes from its "idle" values.
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let cs = state.connection_state.read().await.clone();
        if cs == ConnectionState::Connecting || cs == ConnectionState::Disconnecting {
            return cs;
        }
    }
}

async fn is_current_connect_attempt(state: &Arc<AppState>, attempt: u64) -> bool {
    state.current_connect_attempt() == attempt
        && *state.connection_state.read().await == ConnectionState::Connecting
}

fn compute_reconnect_delay_secs(retries: u64) -> u64 {
    if retries == 0 {
        0
    } else {
        std::cmp::min(1u64 << (retries - 1).min(5), 30)
    }
}

async fn wait_for_reconnect_delay_or_abort(
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

fn should_refresh_stream_snapshot_for_dashboard_event(event_name: &str) -> bool {
    matches!(event_name, "stream_created" | "stream_updated" | "resync")
}

fn consume_sse_line_for_event(line: &str, pending_event: &mut Option<String>) -> Option<String> {
    if line.is_empty() {
        return pending_event.take();
    }
    if line.starts_with(':') {
        return None;
    }
    if let Some(rest) = line.strip_prefix("event:") {
        let event_name = rest.trim();
        if event_name.is_empty() {
            pending_event.take();
        } else {
            *pending_event = Some(event_name.to_owned());
        }
    }
    None
}

fn http_base_url(base_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(base_url).ok()?;
    let scheme = match url.scheme() {
        "ws" => "http",
        "wss" => "https",
        _ => return None,
    };
    let host = url.host_str()?;
    match url.port() {
        Some(port) => Some(format!("{scheme}://{host}:{port}")),
        None => Some(format!("{scheme}://{host}")),
    }
}

async fn run_upstream_dashboard_sse_refresher(state: Arc<AppState>) {
    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            state.logger.log_at(
                UiLogLevel::Warn,
                format!("failed to create upstream SSE client: {e}"),
            );
            return;
        }
    };

    loop {
        if *state.connection_state.read().await != ConnectionState::Connected {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            continue;
        }

        let profile = {
            let db = state.db.lock().await;
            db.load_profile().ok().flatten()
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

        match consume_upstream_dashboard_events(&state, &client, &events_url, &profile.token).await
        {
            Ok(()) => {}
            Err(e) => {
                if *state.connection_state.read().await == ConnectionState::Connected {
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
    client: &reqwest::Client,
    events_url: &str,
    token: &str,
) -> Result<(), String> {
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

    let mut response = response;
    let mut pending_line_bytes: Vec<u8> = Vec::new();
    let mut pending_event: Option<String> = None;

    loop {
        if *state.connection_state.read().await != ConnectionState::Connected {
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
            if let Some(event_name) = consume_sse_line_for_event(&line, &mut pending_event)
                && should_refresh_stream_snapshot_for_dashboard_event(&event_name)
            {
                state.emit_streams_snapshot().await;
            }
        }
    }
}

fn profile_has_connect_credentials(profile: Option<&receiver::db::Profile>) -> bool {
    profile.is_some_and(|profile| {
        !profile.server_url.trim().is_empty() && !profile.token.trim().is_empty()
    })
}

async fn request_reconnect_if_connected(state: &Arc<AppState>) -> bool {
    state.request_reconnect_if_connected().await
}

async fn retry_connect_if_attempt_current(state: &Arc<AppState>, attempt: u64) -> bool {
    if !is_current_connect_attempt(state, attempt).await {
        state.logger.log("Ignoring stale connect retry request");
        return false;
    }
    state.request_retry_connect().await;
    true
}

async fn set_disconnected_if_attempt_current(state: &Arc<AppState>, attempt: u64) -> bool {
    if !is_current_connect_attempt(state, attempt).await {
        state.logger.log("Ignoring stale connect attempt result");
        return false;
    }
    state
        .set_connection_state(ConnectionState::Disconnected)
        .await;
    true
}

// ---------------------------------------------------------------------------
// Helper: perform ReceiverHello / Heartbeat handshake on an open WS.
// Returns (Result<session_id>, Option<ws>) — ws is Some on success.
// ---------------------------------------------------------------------------
#[allow(clippy::type_complexity)]
async fn do_handshake<S>(
    mut ws: S,
    db: &Db,
    ui_tx: &tokio::sync::broadcast::Sender<receiver::ui_events::ReceiverUiEvent>,
) -> (Result<String, receiver::session::SessionError>, Option<S>)
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

    let resume = match db.load_resume_cursors() {
        Ok(cursors) => cursors,
        Err(e) => return (Err(receiver::session::SessionError::Db(e)), None),
    };
    let mut mode = match db.load_receiver_mode() {
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
                Err(e) => return (Err(receiver::session::SessionError::Db(e)), None),
            };
            ReceiverMode::Live {
                streams,
                earliest_epochs: vec![],
            }
        }
        Err(e) => return (Err(receiver::session::SessionError::Db(e)), None),
    };

    if let ReceiverMode::Live {
        ref mut streams,
        ref mut earliest_epochs,
    } = mode
    {
        if streams.is_empty() {
            match db.load_subscriptions() {
                Ok(subs) => {
                    *streams = subs
                        .into_iter()
                        .map(|s| StreamRef {
                            forwarder_id: s.forwarder_id,
                            reader_ip: s.reader_ip,
                        })
                        .collect();
                }
                Err(e) => return (Err(receiver::session::SessionError::Db(e)), None),
            }
        }

        let mut map: HashMap<(String, String), i64> = match db.load_earliest_epochs() {
            Ok(rows) => rows
                .into_iter()
                .map(|(fwd, ip, epoch)| ((fwd, ip), epoch))
                .collect(),
            Err(e) => return (Err(receiver::session::SessionError::Db(e)), None),
        };

        let profile_url = db.load_profile().ok().flatten().map(|p| p.server_url);
        if let Some(url) = profile_url
            && let Ok(server_streams) = receiver::control_api::fetch_server_streams(&url).await
        {
            let server_epoch_by_stream: HashMap<(String, String), i64> = server_streams
                .into_iter()
                .map(|stream| {
                    (
                        (stream.forwarder_id, stream.reader_ip),
                        i64::try_from(stream.stream_epoch).unwrap_or(i64::MAX),
                    )
                })
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
        receiver_id: "receiver-main".to_owned(),
        mode,
        resume,
    });

    let hello_text = match serde_json::to_string(&hello) {
        Ok(t) => t,
        Err(e) => return (Err(receiver::session::SessionError::Json(e)), None),
    };

    if let Err(e) = ws.send(Message::Text(hello_text.into())).await {
        return (Err(receiver::session::SessionError::Ws(e)), None);
    }

    loop {
        let msg = match ws.next().await {
            None => return (Err(receiver::session::SessionError::ConnectionClosed), None),
            Some(Err(e)) => return (Err(receiver::session::SessionError::Ws(e)), None),
            Some(Ok(m)) => m,
        };

        let text = match msg {
            Message::Text(t) => t,
            _ => {
                return (
                    Err(receiver::session::SessionError::UnexpectedFirstMessage),
                    None,
                );
            }
        };

        match serde_json::from_str::<WsMessage>(&text) {
            Ok(WsMessage::Heartbeat(hb)) => {
                info!(session_id = %hb.session_id, "handshake complete");
                return (Ok(hb.session_id), Some(ws));
            }
            Ok(WsMessage::ReceiverModeApplied(applied)) => {
                info!(mode = %applied.mode_summary, streams = applied.resolved_stream_count, "mode applied before heartbeat");
                let _ = ui_tx.send(receiver::ui_events::ReceiverUiEvent::LogEntry {
                    entry: format!(
                        "server applied mode: {} (resolved streams: {})",
                        applied.mode_summary, applied.resolved_stream_count
                    ),
                });
                for warning in applied.warnings {
                    warn!(warning = %warning, "server mode warning");
                    let _ = ui_tx.send(receiver::ui_events::ReceiverUiEvent::LogEntry {
                        entry: format!("server mode warning: {warning}"),
                    });
                }
            }
            Ok(_) => {
                return (
                    Err(receiver::session::SessionError::UnexpectedFirstMessage),
                    None,
                );
            }
            Err(e) => return (Err(receiver::session::SessionError::Json(e)), None),
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: create a broadcast sender that routes through the EventBus.
// run_session_loop takes a broadcast::Sender<ReadEvent>; events sent to it
// are republished through the EventBus so local proxies can subscribe per key.
// ---------------------------------------------------------------------------
fn make_broadcast_sender(bus: &EventBus) -> tokio::sync::broadcast::Sender<rt_protocol::ReadEvent> {
    // We create a dedicated channel. A relay task fans events from this channel
    // into the EventBus so per-stream senders are updated.
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

// ---------------------------------------------------------------------------
// Helper: cancel and join the running WS session task.
// ---------------------------------------------------------------------------
async fn cancel_session(
    task: &mut Option<tokio::task::JoinHandle<()>>,
    cancel_tx: &mut Option<watch::Sender<bool>>,
    logger: &rt_ui_log::UiLogger<receiver::ReceiverUiEvent>,
) {
    // Signal the session loop to stop.
    if let Some(tx) = cancel_tx.take() {
        let _ = tx.send(true);
    }
    // Await the task.
    if let Some(handle) = task.take() {
        // Give the task a moment to exit cleanly; abort if it doesn't.
        let timeout = tokio::time::timeout(std::time::Duration::from_secs(5), handle);
        match timeout.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                logger.log_at(UiLogLevel::Warn, format!("session task panicked: {e}"));
            }
            Err(_) => {
                logger.log_at(
                    UiLogLevel::Warn,
                    "session task did not exit in 5s; continuing",
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helper: reconcile running LocalProxy handles against the desired subscription
// list, stopping removed proxies and starting new ones.
// ---------------------------------------------------------------------------
async fn reconcile_proxies(
    subs: &[Subscription],
    proxies: &mut HashMap<String, LocalProxy>,
    event_bus: &EventBus,
    logger: &rt_ui_log::UiLogger<receiver::ReceiverUiEvent>,
) {
    let assignments = resolve_ports(subs);

    // Determine desired keys.
    let desired_keys: std::collections::HashSet<String> = assignments
        .iter()
        .filter_map(|(k, v)| {
            if matches!(v, PortAssignment::Assigned(_)) {
                Some(k.clone())
            } else {
                None
            }
        })
        .collect();

    // Stop proxies that are no longer wanted.
    proxies.retain(|key, proxy| {
        if !desired_keys.contains(key) {
            info!(key = %key, port = proxy.port, "stopping removed local proxy");
            proxy.shutdown();
            false
        } else {
            true
        }
    });

    // Start new proxies.
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
                        "port collision for {} (port {} used by {}) — skipping",
                        key, wanted, collides_with,
                    ),
                );
                continue;
            }
            None => continue,
        };

        let stream_key_obj =
            receiver::cache::StreamKey::new(sub.forwarder_id.clone(), sub.reader_ip.clone());
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
    use futures_util::{SinkExt, StreamExt};
    use receiver::ui_events::ReceiverUiEvent;
    use rt_protocol::{Heartbeat, ReceiverModeApplied, WsMessage};
    use std::future::Future;
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;
    use tokio_tungstenite::accept_async;
    use tokio_tungstenite::tungstenite::protocol::Message;

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
                device_id: "receiver-main".to_owned(),
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
        let db = Db::open_in_memory().expect("db");
        let (ui_tx, mut ui_rx) = tokio::sync::broadcast::channel::<ReceiverUiEvent>(8);

        let (result, _ws) = do_handshake(ws, &db, &ui_tx).await;
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
    async fn dropped_session_does_not_reconnect_when_disconnect_in_progress() {
        let db = receiver::db::Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);

        state.request_connect().await;
        state
            .set_connection_state(ConnectionState::Disconnecting)
            .await;
        let before_attempt = state.current_connect_attempt();

        let reissued = request_reconnect_if_connected(&state).await;

        assert!(!reissued);
        assert_eq!(state.current_connect_attempt(), before_attempt);
        assert_eq!(
            *state.connection_state.read().await,
            ConnectionState::Disconnecting
        );
    }

    #[tokio::test]
    async fn recoverable_failure_reissues_connect_for_current_attempt() {
        let db = receiver::db::Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);

        state.request_connect().await;
        let attempt = state.current_connect_attempt();

        let retried = retry_connect_if_attempt_current(&state, attempt).await;

        assert!(retried);
        assert!(state.current_connect_attempt() > attempt);
        assert_eq!(
            *state.connection_state.read().await,
            ConnectionState::Connecting
        );
    }

    #[tokio::test]
    async fn recoverable_failure_does_not_reissue_when_attempt_is_stale() {
        let db = receiver::db::Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);

        state.request_connect().await;
        let stale_attempt = state.current_connect_attempt();
        state.request_connect().await;

        let retried = retry_connect_if_attempt_current(&state, stale_attempt).await;

        assert!(!retried);
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

    #[tokio::test]
    async fn reconnect_backoff_wait_aborts_when_state_changes() {
        let db = receiver::db::Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);

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
        let db = receiver::db::Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);

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

    #[tokio::test]
    async fn reconnect_backoff_wait_aborts_when_attempt_becomes_stale() {
        let db = receiver::db::Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);

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
        let db = receiver::db::Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);

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
    fn profile_with_url_and_token_is_required_for_autoconnect() {
        assert!(!profile_has_connect_credentials(None));
        assert!(!profile_has_connect_credentials(Some(
            &receiver::db::Profile {
                server_url: "ws://server".to_owned(),
                token: String::new(),
                update_mode: "check-only".to_owned(),
            }
        )));
        assert!(!profile_has_connect_credentials(Some(
            &receiver::db::Profile {
                server_url: String::new(),
                token: "token".to_owned(),
                update_mode: "check-only".to_owned(),
            }
        )));
        assert!(profile_has_connect_credentials(Some(
            &receiver::db::Profile {
                server_url: "ws://server".to_owned(),
                token: "token".to_owned(),
                update_mode: "check-only".to_owned(),
            }
        )));
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
    fn sse_event_parsing_emits_completed_event_name_on_frame_boundary() {
        let mut pending_event = None;
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
            Some("stream_updated".to_owned())
        );
        assert_eq!(pending_event, None);
    }

    #[test]
    fn sse_event_parsing_ignores_comments_and_data_only_frames() {
        let mut pending_event = None;
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

    #[tokio::test]
    async fn stale_connect_attempt_failure_does_not_force_disconnected() {
        let db = receiver::db::Db::open_in_memory().expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);

        state.request_connect().await;
        let stale_attempt = state.current_connect_attempt();
        state.request_connect().await;

        let transitioned = set_disconnected_if_attempt_current(&state, stale_attempt).await;
        assert!(!transitioned);
        assert_eq!(
            *state.connection_state.read().await,
            ConnectionState::Connecting
        );
    }
}
