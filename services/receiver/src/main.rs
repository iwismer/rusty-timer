use receiver::cache::EventBus;
use receiver::control_api::{AppState, ConnectionState};
use receiver::db::Db;
use receiver::local_proxy::LocalProxy;
use receiver::ports::{resolve_ports, stream_key, PortAssignment};
use receiver::Subscription;
use std::collections::HashMap;
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
    info!("receiver started");
    state.logger.log("Receiver started");

    // -------------------------------------------------------------------------
    // 4. Load profile and restore subscriptions
    // -------------------------------------------------------------------------
    {
        let db = state.db.lock().await;
        if let Ok(Some(profile)) = db.load_profile() {
            *state.upstream_url.write().await = Some(profile.server_url.clone());
            info!(url = %profile.server_url, "restored profile");
        }
    }

    let event_bus = EventBus::new();

    // Start local proxies for any saved subscriptions on startup.
    let initial_subs = {
        let db = state.db.lock().await;
        db.load_subscriptions().unwrap_or_default()
    };
    // Map from stream-key -> LocalProxy handle.
    let mut proxies: HashMap<String, LocalProxy> = HashMap::new();
    reconcile_proxies(&initial_subs, &mut proxies, &event_bus).await;

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
    info!("control API listening on 127.0.0.1:9090");
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
            let checker = match rt_updater::UpdateChecker::new(
                "iwismer",
                "rusty-timer",
                "receiver",
                env!("CARGO_PKG_VERSION"),
            ) {
                Ok(c) => c,
                Err(e) => {
                    warn!(error = %e, "failed to create update checker");
                    return;
                }
            };

            match checker.check().await {
                Ok(rt_updater::UpdateStatus::Available { ref version }) => {
                    info!(
                        current = env!("CARGO_PKG_VERSION"),
                        available = %version,
                        "update available"
                    );
                    *state.update_status.write().await = rt_updater::UpdateStatus::Available {
                        version: version.clone(),
                    };

                    match checker.download(version).await {
                        Ok(path) => {
                            info!(version = %version, "update downloaded and staged");
                            *state.update_status.write().await =
                                rt_updater::UpdateStatus::Downloaded {
                                    version: version.clone(),
                                };
                            *state.staged_update_path.write().await = Some(path);

                            let _ = state
                                .ui_tx
                                .send(receiver::ReceiverUiEvent::UpdateAvailable {
                                    version: version.clone(),
                                    current_version: env!("CARGO_PKG_VERSION").to_owned(),
                                });
                            state.logger.log(format!("Update v{version} available"));
                        }
                        Err(e) => {
                            warn!(error = %e, "update download failed");
                            *state.update_status.write().await = rt_updater::UpdateStatus::Failed {
                                error: e.to_string(),
                            };
                        }
                    }
                }
                Ok(_) => {
                    info!("receiver is up to date");
                }
                Err(e) => {
                    warn!(error = %e, "update check failed");
                    *state.update_status.write().await = rt_updater::UpdateStatus::Failed {
                        error: e.to_string(),
                    };
                }
            }
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
                    info!(n = current_subs.len(), "subscriptions changed, reconciling proxies");
                    reconcile_proxies(&current_subs, &mut proxies, &event_bus).await;
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
                        // Cancel any existing session first.
                        cancel_session(&mut session_task, &mut session_cancel_tx).await;

                        let url_opt = state.upstream_url.read().await.clone();
                        match url_opt {
                            None => {
                                warn!("connect requested but no upstream URL configured");
                                state.logger.log("No upstream URL configured");
                                state.set_connection_state(ConnectionState::Disconnected).await;
                            }
                            Some(base_url) => {
                                // Build the full WS URL from the base URL.
                                let ws_url = format!(
                                    "{}/ws/v1/receivers",
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
                                    warn!("connect requested but no auth token in profile");
                                    state.logger.log("No auth token in profile");
                                    state.set_connection_state(ConnectionState::Disconnected).await;
                                  }
                                  Some(token) => {
                                let ws_request =
                                    receiver::build_authenticated_request(ws_url.as_str(), &token);
                                match ws_request {
                                  Err(e) => {
                                    error!(error = %e, "failed to build WS request");
                                    state.logger.log(format!("Failed to build WS request: {e}"));
                                    state.set_connection_state(ConnectionState::Disconnected).await;
                                  }
                                  Ok(ws_request) => {
                                info!(url = %ws_url, "initiating WS session");
                                match connect_async(ws_request).await {
                                    Err(e) => {
                                        error!(error = %e, "WS connect failed");
                                        state.logger.log(format!("Connection failed: {e}"));
                                        state.set_connection_state(ConnectionState::Disconnected).await;
                                    }
                                    Ok((ws, _)) => {
                                        // Perform the receiver hello / heartbeat handshake.
                                        let (session_result, ws) = {
                                            let db = state.db.lock().await;
                                            do_handshake(ws, &db).await
                                        };
                                        match (session_result, ws) {
                                            (Err(e), _) => {
                                                error!(error = %e, "WS handshake failed");
                                                state.logger.log(format!("Handshake failed: {e}"));
                                                state.set_connection_state(ConnectionState::Disconnected).await;
                                            }
                                            (Ok(session_id), Some(ws)) => {
                                                info!(session_id = %session_id, "WS session established");
                                                state.logger.log(format!("Connected (session {session_id})"));
                                                state.set_connection_state(ConnectionState::Connected).await;
                                                state.emit_streams_snapshot().await;

                                                let (cancel_tx, cancel_rx) =
                                                    watch::channel(false);
                                                let db_arc = Arc::clone(&state.db);
                                                let bus = event_bus.clone();
                                                let st = Arc::clone(&state);
                                                let handle = tokio::spawn(async move {
                                                    let event_tx = make_broadcast_sender(&bus);
                                                    let result = receiver::session::run_session_loop(
                                                        ws,
                                                        session_id,
                                                        db_arc,
                                                        event_tx,
                                                        cancel_rx,
                                                    )
                                                    .await;
                                                    match result {
                                                        Ok(()) => {
                                                            info!("WS session ended normally");
                                                        }
                                                        Err(e) => {
                                                            error!(error = %e, "WS session error");
                                                        }
                                                    }
                                                    let should_disconnect = {
                                                        let cs = st.connection_state.read().await;
                                                        *cs == ConnectionState::Connected
                                                            || *cs == ConnectionState::Disconnecting
                                                    };
                                                    if should_disconnect {
                                                        st.set_connection_state(ConnectionState::Disconnected).await;
                                                        st.emit_streams_snapshot().await;
                                                    }
                                                });
                                                session_task = Some(handle);
                                                session_cancel_tx = Some(cancel_tx);
                                            }
                                            (Ok(_), None) => {
                                                // Should not happen
                                                error!("handshake succeeded but ws was None");
                                                state.logger.log("Handshake succeeded but connection lost");
                                                state.set_connection_state(ConnectionState::Disconnected).await;
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
                        cancel_session(&mut session_task, &mut session_cancel_tx).await;
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
    info!("shutting down receiver");
    cancel_session(&mut session_task, &mut session_cancel_tx).await;
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

// ---------------------------------------------------------------------------
// Helper: perform ReceiverHello / Heartbeat handshake on an open WS.
// Returns (Result<session_id>, Option<ws>) — ws is Some on success.
// ---------------------------------------------------------------------------
#[allow(clippy::type_complexity)]
async fn do_handshake<S>(
    mut ws: S,
    db: &Db,
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
    use rt_protocol::{ReceiverHello, WsMessage};
    use tokio_tungstenite::tungstenite::protocol::Message;

    let resume = match db.load_resume_cursors() {
        Ok(r) => r,
        Err(e) => return (Err(receiver::session::SessionError::Db(e)), None),
    };

    let hello = WsMessage::ReceiverHello(ReceiverHello {
        receiver_id: "receiver-main".to_owned(),
        resume,
    });

    let hello_text = match serde_json::to_string(&hello) {
        Ok(t) => t,
        Err(e) => return (Err(receiver::session::SessionError::Json(e)), None),
    };

    if let Err(e) = ws.send(Message::Text(hello_text.into())).await {
        return (Err(receiver::session::SessionError::Ws(e)), None);
    }

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
            )
        }
    };

    match serde_json::from_str::<WsMessage>(&text) {
        Ok(WsMessage::Heartbeat(hb)) => {
            info!(session_id = %hb.session_id, "handshake complete");
            (Ok(hb.session_id), Some(ws))
        }
        Ok(_) => (
            Err(receiver::session::SessionError::UnexpectedFirstMessage),
            None,
        ),
        Err(e) => (Err(receiver::session::SessionError::Json(e)), None),
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
            Ok(Err(e)) => warn!(error = %e, "session task panicked"),
            Err(_) => warn!("session task did not exit in 5s; continuing"),
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
                warn!(
                    key = %key,
                    wanted = wanted,
                    collides_with = %collides_with,
                    "port collision — skipping proxy for this stream"
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
                error!(key = %key, port = port, error = %e, "failed to bind local proxy");
            }
        }
    }
}
