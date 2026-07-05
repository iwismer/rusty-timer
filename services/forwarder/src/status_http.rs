//! Local status and control HTTP server for the forwarder service.
//!
//! Provides:
//! - `GET /healthz`       — always 200 OK (process is running)
//! - `GET /readyz`        — 200 when local subsystems ready, 503 otherwise
//! - `GET /api/v1/status`  — current forwarder state as JSON
//! - `POST /api/v1/streams/{reader_ip}/reset-epoch`
//!   — bump stream epoch; 200 on success, 404 if unknown
//! - `PUT /api/v1/streams/{reader_ip}/current-epoch/name`
//!   — set epoch name for a reader stream
//! - `GET /api/v1/config` — current config as JSON
//! - `POST /api/v1/config/{section}` — update a config section
//!   (general, auth, journal, status_http, control, update, p2p, ups, readers, screen);
//!   screen config changes require a restart to apply
//! - `POST /api/v1/restart` — trigger graceful restart; 404 if config editing not enabled;
//!   501 on non-Unix platforms
//! - `POST /api/v1/control/restart-service` — trigger graceful service restart
//! - `POST /api/v1/control/restart-device` — trigger host reboot (gated by config)
//! - `POST /api/v1/control/shutdown-device` — trigger host shutdown (gated by config)
//! - `GET /api/v1/readers/{ip}/info`         — reader control info (firmware, clock, etc.)
//! - `POST /api/v1/readers/{ip}/sync-clock`  — synchronize reader clock
//! - `GET /api/v1/readers/{ip}/read-mode`    — current read mode and timeout
//! - `PUT /api/v1/readers/{ip}/read-mode`    — set read mode and timeout
//! - `GET /api/v1/readers/{ip}/tto`          — current TTO reporting state
//! - `PUT /api/v1/readers/{ip}/tto`          — enable or disable TTO bytes in tag reports
//! - `POST /api/v1/readers/{ip}/refresh`     — refresh reader info (re-poll)
//! - `PUT /api/v1/readers/{ip}/recording`    — toggle recording on/off
//! - `POST /api/v1/readers/{ip}/clear-records` — erase stored records
//! - `POST /api/v1/readers/{ip}/download-reads`
//!   — trigger stored-read download from reader; 202 on success, 409 if already running
//! - `GET /api/v1/readers/{ip}/download-reads/progress`
//!   — SSE stream of download progress events
//! - `POST /api/v1/readers/{ip}/reconnect` — trigger immediate reader reconnect (cancels backoff)
//! - `GET /api/v1/logs`   — recent log entries as JSON
//! - `GET /api/v1/events` — SSE stream of all UI events
//! - `GET /update/status`    — current rt-updater status as JSON
//! - `POST /update/apply`    — apply a staged update
//! - `POST /update/check`    — check for updates (respects update mode)
//! - `POST /update/download`  — download an available update (mode-independent)
//! - All other routes fall back to the embedded SvelteKit UI
//!
//! # Readiness contract
//! `/readyz` reflects local prerequisites only (config + SQLite + worker loops).
//! P2P session connectivity does NOT affect readiness.
//!
//! # Security
//! No authentication in v1.

use crate::config_service::{ConfigState, apply_control_action, require_object_payload};
use crate::status_store::{
    ForwarderStatusEvent, ForwarderStatusFeed, ReaderConnectionState, StatusStore, SubsystemStatus,
    UpsStatusState,
};
#[cfg(test)]
use crate::status_store::{ReaderStatus, broadcast_dirty_read_counts};
use crate::storage::journal::Journal;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use rt_updater::UpdateStatus;
use rt_updater::workflow::{RealChecker, run_check, run_download};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify, broadcast};

// ---------------------------------------------------------------------------
// Public config
// ---------------------------------------------------------------------------

/// Configuration for the status HTTP server.
#[derive(Debug, Clone)]
pub struct StatusConfig {
    /// Bind address, e.g. `"127.0.0.1:8080"`.
    pub bind: String,
    /// Forwarder software version (shown in status page).
    pub forwarder_version: String,
}

// ---------------------------------------------------------------------------
// StatusServer handle
// ---------------------------------------------------------------------------

/// Handle to the running status HTTP server.
#[derive(Clone)]
pub struct StatusServer {
    local_addr: SocketAddr,
    store: StatusStore,
}

struct AppState<J: JournalAccess + Send + 'static> {
    store: StatusStore,
    subsystem: Arc<Mutex<SubsystemStatus>>,
    journal: Arc<Mutex<J>>,
    version: Arc<String>,
    config_state: Option<Arc<ConfigState>>,
    restart_signal: Option<Arc<Notify>>,
    ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
    status_event_tx: broadcast::Sender<ForwarderStatusEvent>,
    logger: Arc<rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
    control_clients:
        Arc<std::sync::RwLock<HashMap<String, Arc<crate::reader_control::ControlClient>>>>,
    download_trackers: Arc<
        std::sync::RwLock<
            HashMap<String, Arc<tokio::sync::Mutex<crate::reader_control::DownloadTracker>>>,
        >,
    >,
    reconnect_notifies: Arc<std::sync::RwLock<HashMap<String, Arc<Notify>>>>,
}

impl<J: JournalAccess + Send + 'static> AppState<J> {
    fn reader_control_service(&self) -> crate::reader_control_service::ReaderControlService {
        crate::reader_control_service::ReaderControlService::new(
            self.subsystem.clone(),
            self.control_clients.clone(),
            self.download_trackers.clone(),
            self.reconnect_notifies.clone(),
            self.ui_tx.clone(),
            self.status_event_tx.clone(),
            self.logger.clone(),
        )
    }
}

impl<J: JournalAccess + Send + 'static> Clone for AppState<J> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            subsystem: self.subsystem.clone(),
            journal: self.journal.clone(),
            version: self.version.clone(),
            config_state: self.config_state.clone(),
            restart_signal: self.restart_signal.clone(),
            ui_tx: self.ui_tx.clone(),
            status_event_tx: self.status_event_tx.clone(),
            logger: self.logger.clone(),
            control_clients: self.control_clients.clone(),
            download_trackers: self.download_trackers.clone(),
            reconnect_notifies: self.reconnect_notifies.clone(),
        }
    }
}

impl StatusServer {
    /// Return the bound listen address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Return a clone of the pure status store.
    pub fn store(&self) -> StatusStore {
        self.store.clone()
    }

    /// Return a clone of the internal subsystem status Arc.
    pub fn subsystem_arc(&self) -> Arc<Mutex<SubsystemStatus>> {
        self.store.subsystem_arc()
    }

    /// Return a clone of the UI event broadcast sender.
    pub fn ui_sender(&self) -> tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent> {
        self.store.ui_sender()
    }

    /// Return a read-only status feed for P2P control sessions.
    pub fn status_feed(&self) -> ForwarderStatusFeed {
        self.store.status_feed()
    }

    /// Return the shared reader-control service used by HTTP and P2P control paths.
    pub fn reader_control_service(&self) -> crate::reader_control_service::ReaderControlService {
        self.store.reader_control_service()
    }

    /// Return a clone of the shared UI logger Arc.
    pub fn logger(&self) -> Arc<rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>> {
        self.store.logger()
    }

    /// Mark all local subsystems as ready.
    pub async fn set_ready(&self) {
        self.store.set_ready().await;
    }

    /// Mark that a restart is needed to apply saved config changes.
    pub async fn set_restart_needed(&self) {
        self.store.set_restart_needed().await;
    }

    /// Return whether a restart is needed to apply saved config changes.
    pub async fn restart_needed(&self) -> bool {
        self.store.restart_needed().await
    }

    pub async fn set_p2p_endpoint_id(&self, endpoint_id: String) {
        self.store.set_p2p_endpoint_id(endpoint_id).await;
    }

    /// Update the P2P session state (does not affect readiness).
    pub async fn set_p2p_connected(&self, connected: bool) {
        self.store.set_p2p_connected(connected).await;
    }

    /// Set the forwarder ID (call once at startup).
    pub async fn set_forwarder_id(&self, id: &str) {
        self.store.set_forwarder_id(id).await;
    }

    /// Set the detected local IP (at startup and on reader connect/disconnect).
    pub async fn set_local_ip(&self, ip: Option<String>) {
        self.store.set_local_ip(ip).await;
    }

    /// Set the update mode (controls check-only vs check-and-download behavior).
    pub async fn set_update_mode(&self, mode: rt_updater::UpdateMode) {
        self.store.set_update_mode(mode).await;
    }

    /// Update the current rt-updater status (shown on `/update/status`).
    pub async fn set_update_status(&self, status: UpdateStatus) {
        self.store.set_update_status(status).await;
    }

    /// Record the filesystem path of a downloaded update artifact ready to apply.
    pub async fn set_staged_update_path(&self, path: std::path::PathBuf) {
        self.store.set_staged_update_path(path).await;
    }

    /// Update the UPS status snapshot in the subsystem state.
    pub async fn set_ups_status(&self, state: UpsStatusState) {
        self.store.set_ups_status(state).await;
    }

    pub fn control_clients(
        &self,
    ) -> &Arc<std::sync::RwLock<HashMap<String, Arc<crate::reader_control::ControlClient>>>> {
        self.store.control_clients()
    }

    #[allow(clippy::type_complexity)]
    pub fn download_trackers(
        &self,
    ) -> &Arc<
        std::sync::RwLock<
            HashMap<String, Arc<tokio::sync::Mutex<crate::reader_control::DownloadTracker>>>,
        >,
    > {
        self.store.download_trackers()
    }

    pub fn register_download_tracker(
        &self,
        reader_ip: &str,
        tracker: Arc<tokio::sync::Mutex<crate::reader_control::DownloadTracker>>,
    ) {
        self.store.register_download_tracker(reader_ip, tracker);
    }

    pub fn deregister_download_tracker(&self, reader_ip: &str) {
        self.store.deregister_download_tracker(reader_ip);
    }

    pub fn register_reconnect_notify(&self, reader_ip: &str, notify: Arc<Notify>) {
        self.store.register_reconnect_notify(reader_ip, notify);
    }

    pub fn deregister_reconnect_notify(&self, reader_ip: &str) {
        self.store.deregister_reconnect_notify(reader_ip);
    }

    pub fn reconnect_notifies(&self) -> &Arc<std::sync::RwLock<HashMap<String, Arc<Notify>>>> {
        self.store.reconnect_notifies()
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub fn set_display_sender(
        &mut self,
        tx: tokio::sync::watch::Sender<rt_screen::state::DisplayState>,
    ) {
        self.store.set_display_sender(tx);
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub async fn set_display_name(&self, name: Option<String>) {
        self.store.set_display_name(name).await;
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub async fn set_cpu_temp(&self, temp: Option<f32>) {
        self.store.set_cpu_temp(temp).await;
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub async fn set_cpu_temp_cached(&self, temp: Option<f32>) {
        self.store.set_cpu_temp_cached(temp).await;
    }

    /// Retrieve a clone of the cached reader info for a given reader IP.
    pub async fn get_reader_info(
        &self,
        reader_ip: &str,
    ) -> Option<crate::reader_control::ReaderInfo> {
        self.store.get_reader_info(reader_ip).await
    }

    pub async fn update_reader_info(
        &self,
        reader_ip: &str,
        info: crate::reader_control::ReaderInfo,
    ) {
        self.store.update_reader_info(reader_ip, info).await;
    }

    /// Update reader info only if the reader has not transitioned to Disconnected.
    pub async fn update_reader_info_unless_disconnected(
        &self,
        reader_ip: &str,
        info: crate::reader_control::ReaderInfo,
    ) {
        self.store
            .update_reader_info_unless_disconnected(reader_ip, info)
            .await;
    }

    pub fn register_control_client(
        &self,
        reader_ip: &str,
        client: Arc<crate::reader_control::ControlClient>,
    ) {
        self.store.register_control_client(reader_ip, client);
    }

    pub fn deregister_control_client(&self, reader_ip: &str) {
        self.store.deregister_control_client(reader_ip);
    }

    /// Pre-populate all configured reader IPs as Disconnected.
    pub async fn init_readers(&self, readers: &[(String, u16)]) {
        self.store.init_readers(readers).await;
    }

    /// Seed a reader's total historical count from durable journal state.
    pub async fn set_reader_total(&self, reader_ip: &str, total: i64) {
        self.store.set_reader_total(reader_ip, total).await;
    }

    pub async fn set_reader_epoch_metadata(
        &self,
        reader_ip: &str,
        metadata: crate::storage::journal::CurrentEpochMetadata,
    ) {
        self.store
            .set_reader_epoch_metadata(reader_ip, metadata)
            .await;
    }

    /// Set the current epoch name for a reader and broadcast a ReaderUpdated SSE event.
    pub async fn set_reader_epoch_name(&self, reader_ip: &str, name: Option<String>) {
        self.store.set_reader_epoch_name(reader_ip, name).await;
    }

    /// Update a reader's connection state.
    pub async fn update_reader_state(&self, reader_ip: &str, state: ReaderConnectionState) {
        self.store.update_reader_state(reader_ip, state).await;
    }

    /// Record a successful chip read for a reader.
    pub async fn record_read(&self, reader_ip: &str) {
        self.store.record_read(reader_ip).await;
    }

    /// Start the status HTTP server without a journal (epoch reset returns 404).
    pub async fn start(
        cfg: StatusConfig,
        subsystem: SubsystemStatus,
    ) -> Result<Self, std::io::Error> {
        Self::start_with_journal(cfg, subsystem, Arc::new(Mutex::new(NoJournal))).await
    }

    /// Start the status HTTP server with shared journal access (for epoch reset).
    pub async fn start_with_journal<J: JournalAccess + Send + 'static>(
        cfg: StatusConfig,
        subsystem: SubsystemStatus,
        journal: Arc<Mutex<J>>,
    ) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind(&cfg.bind).await?;
        let local_addr = listener.local_addr()?;

        let store = StatusStore::new(subsystem);
        let state = AppState {
            store: store.clone(),
            subsystem: store.subsystem_arc(),
            journal,
            version: Arc::new(cfg.forwarder_version),
            config_state: None,
            restart_signal: None,
            ui_tx: store.ui_sender(),
            status_event_tx: store.status_event_sender(),
            logger: store.logger(),
            control_clients: store.control_clients().clone(),
            download_trackers: store.download_trackers().clone(),
            reconnect_notifies: store.reconnect_notifies().clone(),
        };

        let app = build_router(state);
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                tracing::error!(error = %err, "status HTTP server fatal error");
            }
        });

        Ok(StatusServer { local_addr, store })
    }

    /// Start the status HTTP server with config editing support.
    pub async fn start_with_config<J: JournalAccess + Send + 'static>(
        cfg: StatusConfig,
        subsystem: SubsystemStatus,
        journal: Arc<Mutex<J>>,
        config_state: Arc<ConfigState>,
        restart_signal: Arc<Notify>,
    ) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind(&cfg.bind).await?;
        let local_addr = listener.local_addr()?;

        let store = StatusStore::new(subsystem);
        let state = AppState {
            store: store.clone(),
            subsystem: store.subsystem_arc(),
            journal,
            version: Arc::new(cfg.forwarder_version),
            config_state: Some(config_state),
            restart_signal: Some(restart_signal),
            ui_tx: store.ui_sender(),
            status_event_tx: store.status_event_sender(),
            logger: store.logger(),
            control_clients: store.control_clients().clone(),
            download_trackers: store.download_trackers().clone(),
            reconnect_notifies: store.reconnect_notifies().clone(),
        };

        let app = build_router(state);
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                tracing::error!(error = %err, "status HTTP server fatal error");
            }
        });

        Ok(StatusServer { local_addr, store })
    }
}

// ---------------------------------------------------------------------------
// JournalAccess trait (for epoch reset, testable with real Journal or NoJournal)
// ---------------------------------------------------------------------------

/// Trait that abstracts journal access for the epoch-reset endpoint.
pub trait JournalAccess {
    /// Bump the epoch for `stream_key`.
    ///
    /// Returns the new current epoch metadata on success, or `Err(NotFound)` if stream unknown.
    fn reset_epoch(
        &mut self,
        stream_key: &str,
    ) -> Result<crate::storage::journal::CurrentEpochMetadata, EpochResetError>;

    /// Return the current epoch metadata for a stream_key, or `None` if stream unknown.
    fn current_epoch_metadata(
        &self,
        stream_key: &str,
    ) -> Result<Option<crate::storage::journal::CurrentEpochMetadata>, String>;

    /// Count total events for a stream_key.
    fn event_count(&self, stream_key: &str) -> Result<i64, String>;
}

#[derive(Debug)]
pub enum EpochResetError {
    NotFound,
    Storage(String),
}

/// Real journal: delegates to `Journal`.
impl JournalAccess for Journal {
    fn reset_epoch(
        &mut self,
        stream_key: &str,
    ) -> Result<crate::storage::journal::CurrentEpochMetadata, EpochResetError> {
        // Get current epoch
        let (current_epoch, _) = self.current_epoch_and_next_seq(stream_key).map_err(|e| {
            // If query_row returns nothing, rusqlite returns QueryReturnedNoRows
            if e.to_string().contains("returned no rows") {
                EpochResetError::NotFound
            } else {
                EpochResetError::Storage(e.to_string())
            }
        })?;
        let new_epoch = current_epoch + 1;
        self.bump_epoch(stream_key, new_epoch)
            .map_err(|e| EpochResetError::Storage(e.to_string()))?;
        self.current_epoch_metadata(stream_key)
            .map_err(|e| EpochResetError::Storage(e.to_string()))?
            .ok_or(EpochResetError::NotFound)
    }

    fn current_epoch_metadata(
        &self,
        stream_key: &str,
    ) -> Result<Option<crate::storage::journal::CurrentEpochMetadata>, String> {
        Journal::current_epoch_metadata(self, stream_key).map_err(|e| e.to_string())
    }

    fn event_count(&self, stream_key: &str) -> Result<i64, String> {
        Journal::event_count(self, stream_key).map_err(|e| e.to_string())
    }
}

/// Sentinel "no journal" implementation: every reset returns NotFound.
struct NoJournal;

impl JournalAccess for NoJournal {
    fn reset_epoch(
        &mut self,
        _stream_key: &str,
    ) -> Result<crate::storage::journal::CurrentEpochMetadata, EpochResetError> {
        Err(EpochResetError::NotFound)
    }

    fn current_epoch_metadata(
        &self,
        _stream_key: &str,
    ) -> Result<Option<crate::storage::journal::CurrentEpochMetadata>, String> {
        Ok(None)
    }

    fn event_count(&self, _stream_key: &str) -> Result<i64, String> {
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn text_response(status: StatusCode, body: impl Into<String>) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain")], body.into()).into_response()
}

fn json_response(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn parse_json_body<T: DeserializeOwned>(body: &Bytes) -> Result<T, String> {
    serde_json::from_slice::<T>(body).map_err(|e| format!("Invalid JSON: {}", e))
}

fn config_not_available() -> Response {
    text_response(StatusCode::NOT_FOUND, "Config editing not available")
}

async fn restart_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    match &state.restart_signal {
        Some(signal) => {
            if cfg!(unix) {
                signal.notify_one();
                json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string())
            } else {
                json_response(
                    StatusCode::NOT_IMPLEMENTED,
                    serde_json::json!({
                        "ok": false,
                        "error": "restart not supported on non-unix platforms"
                    })
                    .to_string(),
                )
            }
        }
        None => config_not_available(),
    }
}

fn control_action_error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|json| {
            json.get("error")
                .and_then(|value| value.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| body.to_owned())
}

fn log_control_action_failure(
    logger: &rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>,
    action: &str,
    status_code: u16,
    body: &str,
) {
    let error = control_action_error_message(body);
    logger.log_at(
        rt_ui_log::UiLogLevel::Error,
        format!(
            "control action '{}' failed (HTTP {}): {}",
            action, status_code, error
        ),
    );
}

async fn control_restart_service_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    match apply_control_action(
        "restart_service",
        None,
        state.restart_signal.as_ref(),
        Some(&state.logger),
    )
    .await
    {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => {
            log_control_action_failure(
                state.logger.as_ref(),
                "restart_service",
                status_code,
                &body,
            );
            json_response(
                StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                body,
            )
        }
    }
}

async fn control_restart_device_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    match apply_control_action(
        "restart_device",
        Some(&cs),
        state.restart_signal.as_ref(),
        Some(&state.logger),
    )
    .await
    {
        Ok(()) => json_response(
            StatusCode::OK,
            serde_json::json!({"ok": true, "status": "restart_device_scheduled"}).to_string(),
        ),
        Err((status_code, body)) => {
            log_control_action_failure(state.logger.as_ref(), "restart_device", status_code, &body);
            json_response(
                StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                body,
            )
        }
    }
}

async fn control_shutdown_device_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    match apply_control_action(
        "shutdown_device",
        Some(&cs),
        state.restart_signal.as_ref(),
        Some(&state.logger),
    )
    .await
    {
        Ok(()) => json_response(
            StatusCode::OK,
            serde_json::json!({"ok": true, "status": "shutdown_device_scheduled"}).to_string(),
        ),
        Err((status_code, body)) => {
            log_control_action_failure(
                state.logger.as_ref(),
                "shutdown_device",
                status_code,
                &body,
            );
            json_response(
                StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                body,
            )
        }
    }
}

fn get_config_state<J: JournalAccess + Send + 'static>(
    state: &AppState<J>,
) -> Option<Arc<ConfigState>> {
    state.config_state.clone()
}

#[derive(serde::Serialize)]
struct StatusJsonResponse {
    forwarder_id: String,
    version: String,
    ready: bool,
    ready_reason: Option<String>,
    p2p_connected: bool,
    restart_needed: bool,
    ups_status: Option<UpsStatusState>,
    server: crate::status_store::ServerDeviceStatus,
    /// Total local-fanout messages dropped because consumers lagged.
    fanout_dropped_total: u64,
    readers: Vec<ReaderStatusJson>,
}

#[derive(serde::Serialize)]
struct ReaderStatusJson {
    ip: String,
    state: String,
    reads_session: u64,
    reads_total: i64,
    last_seen_secs: Option<u64>,
    local_port: u16,
    current_epoch: Option<i64>,
    current_epoch_created_unix_ms: Option<i64>,
    current_epoch_name: Option<String>,
    reader_info: Option<crate::reader_control::ReaderInfo>,
}

async fn status_json_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let ss = state.subsystem.lock().await;
    // Served from the cache maintained by `server_status_task`; the handler
    // itself performs no outbound I/O so local status latency never depends
    // on WAN state. Before the first poll completes (or when no poll task
    // runs), serve the not-configured shape with `checked_unix_ms: None`.
    let server = ss
        .server_status()
        .cloned()
        .unwrap_or_else(crate::status_store::ServerDeviceStatus::not_configured);
    let forwarder_id = ss.forwarder_id.clone();
    let ready = ss.is_ready();
    let ready_reason = ss.reason.clone();
    let p2p_connected = ss.p2p_connected();
    let restart_needed = ss.restart_needed();
    let ups_status = ss.ups_status().cloned();
    let reader_snapshots: Vec<_> = ss
        .readers
        .iter()
        .map(|(ip, r)| {
            let state_str = match r.state {
                ReaderConnectionState::Connected => "connected",
                ReaderConnectionState::Connecting => "connecting",
                ReaderConnectionState::Disconnected => "disconnected",
            };
            (
                ip.clone(),
                state_str.to_owned(),
                r.reads_since_restart,
                r.reads_total,
                r.last_seen.map(|t| t.elapsed().as_secs()),
                r.local_port,
                r.current_epoch_name.clone(),
                r.reader_info.clone(),
            )
        })
        .collect();
    drop(ss);

    let journal = state.journal.lock().await;
    let mut readers: Vec<_> = Vec::with_capacity(reader_snapshots.len());
    for (
        ip,
        state,
        reads_session,
        reads_total,
        last_seen_secs,
        local_port,
        current_epoch_name,
        reader_info,
    ) in reader_snapshots
    {
        let epoch = journal.current_epoch_metadata(&ip).ok().flatten();
        readers.push(ReaderStatusJson {
            ip,
            state,
            reads_session,
            reads_total,
            last_seen_secs,
            local_port,
            current_epoch: epoch.map(|metadata| metadata.epoch),
            current_epoch_created_unix_ms: epoch.and_then(|metadata| metadata.created_unix_ms),
            current_epoch_name,
            reader_info,
        });
    }
    readers.sort_by(|a, b| a.ip.cmp(&b.ip));

    let resp = StatusJsonResponse {
        forwarder_id,
        version: (*state.version).clone(),
        ready,
        ready_reason,
        p2p_connected,
        restart_needed,
        ups_status,
        server,
        fanout_dropped_total: state.store.fanout_dropped_total(),
        readers,
    };

    match serde_json::to_string(&resp) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(e) => {
            tracing::error!("status JSON serialization failed: {e}");
            json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                r#"{"error":"serialization error"}"#.to_owned(),
            )
        }
    }
}

/// Returns the current display state as JSON, matching the `DisplayState` schema
/// from `rt-screen`. Used by the desktop display simulator to render live forwarder data.
// The `return` in the backend-enabled branch is required because the
// `#[cfg(not(any(...)))]` fallback block follows it; only one is compiled.
#[allow(clippy::needless_return)]
async fn display_state_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> axum::Json<serde_json::Value> {
    #[cfg(any(feature = "eink", feature = "lcd"))]
    {
        return axum::Json(
            serde_json::to_value(state.store.display_state().await)
                .expect("display state serializes"),
        );
    }

    #[cfg(not(any(feature = "eink", feature = "lcd")))]
    {
        let ss = state.subsystem.lock().await;
        let readers: Vec<serde_json::Value> = ss
            .readers
            .iter()
            .map(|(addr, r)| {
                let ip = addr.rsplit_once(':').map_or(addr.as_str(), |(ip, _)| ip);
                let state_str = match r.state {
                    ReaderConnectionState::Connected => "connected",
                    ReaderConnectionState::Connecting => "connecting",
                    ReaderConnectionState::Disconnected => "disconnected",
                };
                let drift_ms = r
                    .reader_info
                    .as_ref()
                    .and_then(|info| info.clock.as_ref())
                    .map(|c| c.drift_ms);
                serde_json::json!({
                    "ip": ip,
                    "state": state_str,
                    "drift_ms": drift_ms,
                    "session_reads": r.reads_since_restart,
                })
            })
            .collect();

        let total_reads: u64 = ss.readers.values().map(|r| r.reads_since_restart).sum();

        axum::Json(serde_json::json!({
            "forwarder_name": null,
            "local_ip": ss.local_ip,
            "p2p_connected": ss.p2p_connected(),
            "readers": readers,
            "total_reads": total_reads,
            "cpu_temp_celsius": null,
            "battery": null,
        }))
    }
}

async fn logs_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "entries": state.logger.entries() }))
}

async fn events_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> axum::response::sse::Sse<
    impl futures_util::stream::Stream<
        Item = Result<axum::response::sse::Event, std::convert::Infallible>,
    >,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use tokio_stream::{StreamExt, wrappers::BroadcastStream};

    let rx = state.ui_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(event) => {
            let event_type = match &event {
                crate::ui_events::ForwarderUiEvent::StatusChanged { .. } => "status_changed",
                crate::ui_events::ForwarderUiEvent::ReaderUpdated { .. } => "reader_updated",
                crate::ui_events::ForwarderUiEvent::LogEntry { .. } => "log_entry",
                crate::ui_events::ForwarderUiEvent::UpdateStatusChanged { .. } => {
                    "update_status_changed"
                }
                crate::ui_events::ForwarderUiEvent::ReaderInfoUpdated { .. } => {
                    "reader_info_updated"
                }
                crate::ui_events::ForwarderUiEvent::UpsStatusChanged { .. } => "ups_status_changed",
                crate::ui_events::ForwarderUiEvent::ServerStatusChanged { .. } => {
                    "server_status_changed"
                }
            };
            match serde_json::to_string(&event) {
                Ok(json) => Some(Ok(Event::default().event(event_type).data(json))),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to serialize SSE event");
                    None
                }
            }
        }
        Err(_) => Some(Ok(Event::default().event("resync").data("{}"))),
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
}

// ---------------------------------------------------------------------------
// Reader control API handlers
// ---------------------------------------------------------------------------

/// GET /api/v1/readers/{ip}/info
async fn reader_info_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    let ss = state.subsystem.lock().await;
    match ss.readers.get(&ip) {
        Some(r) => match &r.reader_info {
            Some(info) => match serde_json::to_string(info) {
                Ok(json) => json_response(StatusCode::OK, json),
                Err(e) => {
                    tracing::error!(error = %e, "failed to serialize reader info");
                    json_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        r#"{"error":"internal serialization error"}"#.to_owned(),
                    )
                }
            },
            None => StatusCode::NO_CONTENT.into_response(),
        },
        None => text_response(StatusCode::NOT_FOUND, "unknown reader"),
    }
}

/// POST /api/v1/readers/{ip}/sync-clock
///
/// Minimizes clock drift by:
/// 1. Probing RTT to estimate one-way network latency
/// 2. Rounding the projected rollover time to the nearest whole-second boundary
/// 3. Delaying the SET command so the rollover aligns with the target second
///
/// SET_DATE_TIME resets the centisecond counter to ~52 (520ms) and applies the
/// new second value when cs next rolls over from 99 → 0, which takes ~480ms.
/// The reader's unsolicited 0x4c frame confirms a 500ms sync delay. We compute
/// the ideal send time so that: send_time + one_way + SYNC_DELAY = S.000,
/// reducing drift from ±500ms (pure rounding) to ~25ms (RTT estimation error).
async fn sync_clock_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    match state.reader_control_service().sync_clock(&ip).await {
        Ok(info) => {
            let clock = info.clock.as_ref();
            json_response(
                StatusCode::OK,
                serde_json::json!({
                    "reader_clock": clock.map(|c| c.reader_clock.clone()),
                    "clock_drift_ms": clock.map(|c| c.drift_ms),
                })
                .to_string(),
            )
        }
        Err(e) if e == "reader not connected" => {
            text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected")
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e}).to_string(),
        ),
    }
}

#[cfg(test)]
async fn update_cached_reader_info<J: JournalAccess + Send + 'static>(
    state: &AppState<J>,
    ip: &str,
    info: crate::reader_control::ReaderInfo,
) {
    {
        let mut ss = state.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(ip) {
            if r.state == ReaderConnectionState::Disconnected {
                tracing::debug!(
                    ip,
                    "dropping cached reader info update for disconnected reader"
                );
                return;
            }
            r.reader_info = Some(info.clone());
        } else {
            tracing::warn!(reader_ip = %ip, "update_cached_reader_info: reader not found in status map, skipping broadcast");
            return;
        }
    }
    let _ = state
        .ui_tx
        .send(crate::ui_events::ForwarderUiEvent::ReaderInfoUpdated {
            ip: ip.to_owned(),
            info: info.clone(),
        });
    let _ = state
        .status_event_tx
        .send(ForwarderStatusEvent::ReaderInfo {
            stream_id: ip.to_owned(),
            info,
        });
}

/// GET /api/v1/readers/{ip}/read-mode
async fn get_read_mode_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    match state.reader_control_service().get_read_mode(&ip).await {
        Ok((mode, timeout)) => json_response(
            StatusCode::OK,
            serde_json::json!({"mode": mode.as_str(), "timeout": timeout}).to_string(),
        ),
        Err(e) if e == "reader not connected" => {
            text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected")
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e}).to_string(),
        ),
    }
}

#[derive(serde::Deserialize)]
struct SetReadModeBody {
    mode: String,
    #[serde(default = "default_timeout")]
    timeout: u8,
}
fn default_timeout() -> u8 {
    5
}

#[derive(serde::Deserialize)]
struct SetTtoBody {
    enabled: bool,
}

#[derive(serde::Deserialize)]
struct SetRecordingBody {
    enabled: bool,
}

/// PUT /api/v1/readers/{ip}/read-mode
async fn set_read_mode_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
    axum::Json(body): axum::Json<SetReadModeBody>,
) -> Response {
    let mode = match crate::reader_control_service::parse_native_read_mode(&body.mode) {
        Ok(mode) => mode,
        Err(e) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"error": e}).to_string(),
            );
        }
    };
    match state
        .reader_control_service()
        .set_read_mode(&ip, mode, body.timeout)
        .await
    {
        Ok(_) => json_response(
            StatusCode::OK,
            serde_json::json!({"mode": mode.as_str()}).to_string(),
        ),
        Err(e) if e == "reader not connected" => {
            text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected")
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e}).to_string(),
        ),
    }
}

/// GET /api/v1/readers/{ip}/tto
async fn get_tto_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    match state.reader_control_service().get_tto(&ip).await {
        Ok(enabled) => json_response(
            StatusCode::OK,
            serde_json::json!({"enabled": enabled}).to_string(),
        ),
        Err(e) if e == "reader not connected" => {
            text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected")
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e}).to_string(),
        ),
    }
}

/// PUT /api/v1/readers/{ip}/tto
async fn set_tto_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
    axum::Json(body): axum::Json<SetTtoBody>,
) -> Response {
    match state
        .reader_control_service()
        .set_tto(&ip, body.enabled)
        .await
    {
        Ok(info) => json_response(
            StatusCode::OK,
            serde_json::json!({"enabled": info.tto_enabled.unwrap_or(body.enabled)}).to_string(),
        ),
        Err(e) if e == "reader not connected" => {
            text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected")
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e}).to_string(),
        ),
    }
}

/// POST /api/v1/readers/{ip}/refresh
async fn refresh_handler_reader<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    match state.reader_control_service().refresh(&ip).await {
        Ok(info) => match serde_json::to_string(&info) {
            Ok(json) => json_response(StatusCode::OK, json),
            Err(e) => {
                tracing::error!(error = %e, "failed to serialize reader info");
                json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    r#"{"error":"internal serialization error"}"#.to_owned(),
                )
            }
        },
        Err(e) if e == "reader not connected" => {
            text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected")
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e}).to_string(),
        ),
    }
}

/// POST /api/v1/readers/{ip}/clear-records
async fn clear_records_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    match state.reader_control_service().clear_records(&ip).await {
        Ok(()) => json_response(StatusCode::OK, "{\"ok\":true}".to_owned()),
        Err(e) if e == "reader not connected" => {
            text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected")
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e}).to_string(),
        ),
    }
}

/// PUT /api/v1/readers/{ip}/recording
async fn set_recording_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
    axum::Json(body): axum::Json<SetRecordingBody>,
) -> Response {
    match state
        .reader_control_service()
        .set_recording(&ip, body.enabled)
        .await
    {
        Ok(info) => json_response(
            StatusCode::OK,
            serde_json::json!({"recording": info.recording.unwrap_or(false)}).to_string(),
        ),
        Err(e) if e == "reader not connected" => {
            text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected")
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e}).to_string(),
        ),
    }
}

/// POST /api/v1/readers/{ip}/reconnect
async fn reconnect_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    match state.reader_control_service().reconnect(&ip).await {
        Ok(()) => json_response(StatusCode::OK, r#"{"ok":true}"#.to_string()),
        Err(_) => json_response(
            StatusCode::NOT_FOUND,
            r#"{"error":"reader not found"}"#.to_string(),
        ),
    }
}

/// POST /api/v1/readers/{ip}/download-reads
async fn download_reads_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    match state.reader_control_service().start_download(&ip).await {
        Ok(estimated_reads) => json_response(
            StatusCode::ACCEPTED,
            serde_json::json!({"status": "started", "estimated_reads": estimated_reads})
                .to_string(),
        ),
        Err(e) if e == "reader not connected" => json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"reader not connected"}"#.to_string(),
        ),
        Err(e) if e == "download already in progress" => json_response(
            StatusCode::CONFLICT,
            r#"{"error":"download already in progress"}"#.to_string(),
        ),
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e}).to_string(),
        ),
    }
}

async fn download_progress_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    let tracker = {
        state
            .download_trackers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    let Some(tracker) = tracker else {
        return json_response(
            StatusCode::NOT_FOUND,
            r#"{"error":"reader not connected"}"#.to_string(),
        );
    };

    // Lock tracker, capture initial state if terminal, and subscribe
    let (initial_event, mut rx) = {
        let dt = tracker.lock().await;
        let initial = match dt.state() {
            crate::reader_control::DownloadState::Idle => {
                Some(crate::reader_control::DownloadEvent::Idle)
            }
            crate::reader_control::DownloadState::Complete => {
                Some(crate::reader_control::DownloadEvent::Complete {
                    reads_received: dt.reads_received(),
                })
            }
            crate::reader_control::DownloadState::Error(msg) => {
                Some(crate::reader_control::DownloadEvent::Error {
                    message: msg.clone(),
                })
            }
            crate::reader_control::DownloadState::Starting
            | crate::reader_control::DownloadState::Downloading => None,
        };
        let rx = dt.subscribe();
        (initial, rx)
    };

    let stream = async_stream::stream! {
        // If there's an initial terminal event, yield it and close
        if let Some(evt) = initial_event {
            let json = serde_json::to_string(&evt)
                .unwrap_or_else(|e| serde_json::json!({"state": "error", "message": format!("serialize: {e}")}).to_string());
            yield Ok::<_, Infallible>(SseEvent::default().data(json));
            return;
        }

        // Stream events from the broadcast channel (5-minute inactivity timeout)
        loop {
            let recv_result = tokio::select! {
                result = rx.recv() => result,
                _ = tokio::time::sleep(std::time::Duration::from_secs(300)) => {
                    let timeout_json = serde_json::json!({
                        "state": "error",
                        "message": "download progress stream timed out"
                    }).to_string();
                    yield Ok::<_, Infallible>(SseEvent::default().data(timeout_json));
                    return;
                }
            };
            match recv_result {
                Ok(evt) => {
                    let is_terminal = matches!(
                        evt,
                        crate::reader_control::DownloadEvent::Complete { .. }
                            | crate::reader_control::DownloadEvent::Error { .. }
                    );
                    let json = serde_json::to_string(&evt)
                        .unwrap_or_else(|e| serde_json::json!({"state": "error", "message": format!("serialize: {e}")}).to_string());
                    yield Ok::<_, Infallible>(SseEvent::default().data(json));
                    if is_terminal {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("download SSE client lagged, skipped {n} events");
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                    let err_json = serde_json::json!({
                        "state": "error",
                        "message": "download tracker closed unexpectedly"
                    }).to_string();
                    yield Ok::<_, Infallible>(SseEvent::default().data(err_json));
                    return;
                }
            }
        }
    };

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn build_router<J: JournalAccess + Send + 'static>(state: AppState<J>) -> Router {
    let router = Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/readyz", get(readyz_handler::<J>))
        .route(
            "/api/v1/streams/{reader_ip}/reset-epoch",
            post(reset_epoch_handler::<J>),
        )
        .route(
            "/api/v1/streams/{reader_ip}/current-epoch/name",
            put(set_current_epoch_name_handler::<J>),
        )
        .route("/update/status", get(update_status_handler::<J>))
        .route("/update/apply", post(update_apply_handler::<J>))
        .route("/update/check", post(update_check_handler::<J>))
        .route("/update/download", post(update_download_handler::<J>))
        .route("/api/v1/config", get(config_json_handler::<J>))
        .route(
            "/api/v1/config/general",
            post(post_config_general_handler::<J>),
        )
        .route("/api/v1/config/auth", post(post_config_auth_handler::<J>))
        .route(
            "/api/v1/config/journal",
            post(post_config_journal_handler::<J>),
        )
        .route(
            "/api/v1/config/status_http",
            post(post_config_status_http_handler::<J>),
        )
        .route(
            "/api/v1/config/control",
            post(post_config_control_handler::<J>),
        )
        .route(
            "/api/v1/config/update",
            post(post_config_update_handler::<J>),
        )
        .route("/api/v1/config/p2p", post(post_config_p2p_handler::<J>))
        .route("/api/v1/config/ups", post(post_config_ups_handler::<J>))
        .route(
            "/api/v1/config/readers",
            post(post_config_readers_handler::<J>),
        );

    #[cfg(any(feature = "eink", feature = "lcd"))]
    let router = router.route(
        "/api/v1/config/screen",
        post(post_config_screen_handler::<J>),
    );

    router
        .route("/api/v1/restart", post(restart_handler::<J>))
        .route(
            "/api/v1/control/restart-service",
            post(control_restart_service_handler::<J>),
        )
        .route(
            "/api/v1/control/restart-device",
            post(control_restart_device_handler::<J>),
        )
        .route(
            "/api/v1/control/shutdown-device",
            post(control_shutdown_device_handler::<J>),
        )
        .route("/api/v1/status", get(status_json_handler::<J>))
        .route("/api/v1/display-state", get(display_state_handler::<J>))
        .route("/api/v1/logs", get(logs_handler::<J>))
        .route("/api/v1/events", get(events_handler::<J>))
        .route("/api/v1/readers/{ip}/info", get(reader_info_handler::<J>))
        .route(
            "/api/v1/readers/{ip}/sync-clock",
            post(sync_clock_handler::<J>),
        )
        .route(
            "/api/v1/readers/{ip}/read-mode",
            get(get_read_mode_handler::<J>).put(set_read_mode_handler::<J>),
        )
        .route(
            "/api/v1/readers/{ip}/tto",
            get(get_tto_handler::<J>).put(set_tto_handler::<J>),
        )
        .route(
            "/api/v1/readers/{ip}/refresh",
            post(refresh_handler_reader::<J>),
        )
        .route(
            "/api/v1/readers/{ip}/clear-records",
            post(clear_records_handler::<J>),
        )
        .route(
            "/api/v1/readers/{ip}/recording",
            put(set_recording_handler::<J>),
        )
        .route(
            "/api/v1/readers/{ip}/download-reads",
            post(download_reads_handler::<J>),
        )
        .route(
            "/api/v1/readers/{ip}/download-reads/progress",
            get(download_progress_handler::<J>),
        )
        .route(
            "/api/v1/readers/{ip}/reconnect",
            post(reconnect_handler::<J>),
        )
        .fallback(crate::ui_server::serve_ui)
        .with_state(state)
}

async fn healthz_handler() -> &'static str {
    "ok"
}

async fn readyz_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let ss = state.subsystem.lock().await;
    if ss.is_ready() {
        text_response(StatusCode::OK, "ready")
    } else {
        let reason = ss.reason.clone().unwrap_or_else(|| "not ready".to_owned());
        text_response(StatusCode::SERVICE_UNAVAILABLE, reason)
    }
}

async fn reset_epoch_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(reader_ip): Path<String>,
) -> Response {
    let result = state.journal.lock().await.reset_epoch(&reader_ip);
    match result {
        Ok(metadata) => {
            if let Some(reader) = state.subsystem.lock().await.readers.get_mut(&reader_ip) {
                reader.current_epoch = Some(metadata.epoch);
                reader.current_epoch_created_unix_ms = metadata.created_unix_ms;
                reader.current_epoch_name = None;
                let _ = state
                    .ui_tx
                    .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                        ip: reader_ip.clone(),
                        state: (&reader.state).into(),
                        reads_session: reader.reads_since_restart,
                        reads_total: reader.reads_total,
                        last_seen_secs: reader.last_seen.map(|t| t.elapsed().as_secs()),
                        local_port: reader.local_port,
                        current_epoch_name: None,
                    });
                let _ = state
                    .status_event_tx
                    .send(ForwarderStatusEvent::ReaderStatus {
                        stream_id: reader_ip.clone(),
                        status: reader.clone(),
                    });
            }
            state
                .logger
                .log(format!("epoch reset for {} via API", reader_ip));
            let body = serde_json::json!({
                "new_epoch": metadata.epoch,
                "created_unix_ms": metadata.created_unix_ms,
            })
            .to_string();
            json_response(StatusCode::OK, body)
        }
        Err(EpochResetError::NotFound) => text_response(StatusCode::NOT_FOUND, "stream not found"),
        Err(EpochResetError::Storage(e)) => text_response(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

async fn set_current_epoch_name_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(reader_ip): Path<String>,
    body: Bytes,
) -> Response {
    let payload = match parse_json_body::<serde_json::Value>(&body) {
        Ok(value) => value,
        Err(error) => return text_response(StatusCode::BAD_REQUEST, error),
    };
    if let Err((status, message)) = require_object_payload(&payload) {
        return text_response(status_from_u16_or_internal(status), message);
    }

    let normalized_name = match payload.get("name") {
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(name)) => {
            let trimmed = name.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        }
        Some(_) => return text_response(StatusCode::BAD_REQUEST, "name must be a string or null"),
        None => return text_response(StatusCode::BAD_REQUEST, "name is required"),
    };

    let mut ss = state.subsystem.lock().await;
    let Some(reader) = ss.readers.get_mut(&reader_ip) else {
        return text_response(StatusCode::NOT_FOUND, "reader not found");
    };

    reader.current_epoch_name = normalized_name;
    let _ = state
        .ui_tx
        .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
            ip: reader_ip.to_owned(),
            state: (&reader.state).into(),
            reads_session: reader.reads_since_restart,
            reads_total: reader.reads_total,
            last_seen_secs: reader.last_seen.map(|t| t.elapsed().as_secs()),
            local_port: reader.local_port,
            current_epoch_name: reader.current_epoch_name.clone(),
        });
    state.logger.log(format!(
        "set current epoch name for {} via local API",
        reader_ip
    ));
    drop(ss);

    json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string())
}

fn status_from_u16_or_internal(status: u16) -> StatusCode {
    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

async fn update_status_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let update_status = state.store.update_status().await;
    let body = serde_json::to_string(&update_status)
        .unwrap_or_else(|_| r#"{"status":"failed","error":"serialization error"}"#.to_owned());
    json_response(StatusCode::OK, body)
}

async fn update_apply_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    match state.store.staged_update_path().await {
        Some(path) => {
            if apply_via_restart_enabled() {
                schedule_process_restart();
                json_response(StatusCode::OK, r#"{"status":"restarting"}"#.to_owned())
            } else {
                let store = state.store.clone();
                let restart = state.restart_signal.clone();
                tokio::spawn(async move {
                    match tokio::task::spawn_blocking(move || {
                        rt_updater::UpdateChecker::apply_update(&path)
                    })
                    .await
                    {
                        Ok(Ok(())) => {
                            if let Some(notify) = restart.as_ref() {
                                notify.notify_one();
                            }
                        }
                        Ok(Err(e)) => {
                            tracing::error!(error = %e, "update apply failed");
                            store
                                .set_update_status(rt_updater::UpdateStatus::Failed {
                                    error: e.to_string(),
                                })
                                .await;
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "update apply task failed");
                            store
                                .set_update_status(rt_updater::UpdateStatus::Failed {
                                    error: e.to_string(),
                                })
                                .await;
                        }
                    }
                });
                json_response(StatusCode::OK, r#"{"status":"applying"}"#.to_owned())
            }
        }
        None => json_response(
            StatusCode::NOT_FOUND,
            r#"{"error":"no update staged"}"#.to_owned(),
        ),
    }
}

async fn update_check_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let update_mode = state.store.update_mode().await;

    let checker = match rt_updater::UpdateChecker::new(
        "iwismer",
        "rusty-timer",
        "forwarder",
        env!("CARGO_PKG_VERSION"),
    ) {
        Ok(c) => RealChecker::with_stage_root(c, crate::updater_stage_root_dir()),
        Err(e) => {
            let status = rt_updater::UpdateStatus::Failed {
                error: e.to_string(),
            };
            state.store.set_update_status(status.clone()).await;
            let body = serde_json::to_string(&status).unwrap_or_else(|_| {
                r#"{"status":"failed","error":"serialization error"}"#.to_owned()
            });
            return json_response(StatusCode::INTERNAL_SERVER_ERROR, body);
        }
    };

    let workflow_state = state.store.workflow_state();
    let status = run_check(&workflow_state, &checker, update_mode).await;
    let body = serde_json::to_string(&status)
        .unwrap_or_else(|_| r#"{"status":"failed","error":"serialization error"}"#.to_owned());
    json_response(StatusCode::OK, body)
}

async fn update_download_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let checker = match rt_updater::UpdateChecker::new(
        "iwismer",
        "rusty-timer",
        "forwarder",
        env!("CARGO_PKG_VERSION"),
    ) {
        Ok(c) => RealChecker::with_stage_root(c, crate::updater_stage_root_dir()),
        Err(e) => {
            let status = rt_updater::UpdateStatus::Failed {
                error: e.to_string(),
            };
            state.store.set_update_status(status.clone()).await;
            let body = serde_json::to_string(&status).unwrap_or_else(|_| {
                r#"{"status":"failed","error":"serialization error"}"#.to_owned()
            });
            return json_response(StatusCode::INTERNAL_SERVER_ERROR, body);
        }
    };

    let workflow_state = state.store.workflow_state();
    match run_download(&workflow_state, &checker).await {
        Ok(status) => {
            let body = serde_json::to_string(&status).unwrap_or_default();
            json_response(StatusCode::OK, body)
        }
        Err(status) => {
            let body = serde_json::to_string(&status).unwrap_or_default();
            json_response(StatusCode::CONFLICT, body)
        }
    }
}

async fn config_json_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };

    match crate::config_service::read_config_json(&cs, &state.subsystem).await {
        Ok((json_value, _restart_needed)) => {
            let json_str = serde_json::to_string(&json_value).unwrap_or_else(|e| {
                serde_json::json!({"ok": false, "error": format!("JSON serialize error: {}", e)})
                    .to_string()
            });
            json_response(StatusCode::OK, json_str)
        }
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_section_handler<J: JournalAccess + Send + 'static>(
    section: &'static str,
    state: AppState<J>,
    body: Bytes,
    logger: Option<&rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let payload: serde_json::Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            );
        }
    };

    match crate::config_service::apply_section_update(
        section,
        &payload,
        &cs,
        &state.subsystem,
        &state.ui_tx,
        logger,
    )
    .await
    {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_general_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("general", state, body, None).await
}

async fn post_config_auth_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("auth", state, body, None).await
}

async fn post_config_journal_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("journal", state, body, None).await
}

async fn post_config_status_http_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("status_http", state, body, None).await
}

async fn post_config_ups_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("ups", state, body, None).await
}

async fn post_config_readers_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("readers", state, body, None).await
}

async fn post_config_control_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let logger = state.logger.clone();
    post_config_section_handler("control", state, body, Some(&logger)).await
}

async fn post_config_update_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("update", state, body, None).await
}

async fn post_config_p2p_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("p2p", state, body, None).await
}

#[cfg(any(feature = "eink", feature = "lcd"))]
async fn post_config_screen_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    // This endpoint can change hardware GPIO/SPI pins; keep the unauthenticated
    // status HTTP server bound to a trusted interface.
    post_config_section_handler("screen", state, body, None).await
}

fn apply_via_restart_enabled() -> bool {
    apply_via_restart_from_env(std::env::var("RT_FORWARDER_UPDATE_APPLY_VIA_RESTART").ok())
}

fn apply_via_restart_from_env(value: Option<String>) -> bool {
    value.is_some_and(|raw| {
        let normalized = raw.trim().to_ascii_lowercase();
        matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
    })
}

#[cfg(not(test))]
fn schedule_process_restart() {
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        std::process::exit(1);
    });
}

#[cfg(test)]
fn schedule_process_restart() {}

#[cfg(test)]
mod tests {
    use super::*;
    use ipico_core::control::{self, Command, TagMessageFormat};
    use rt_updater::workflow::{Checker, run_check, run_download};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::time::{Duration, sleep};

    struct FakeChecker {
        check_result: Result<UpdateStatus, String>,
        download_result: Result<std::path::PathBuf, String>,
        download_calls: Arc<AtomicUsize>,
    }

    impl Checker for FakeChecker {
        fn check<'a>(
            &'a self,
        ) -> Pin<Box<dyn Future<Output = Result<UpdateStatus, String>> + Send + 'a>> {
            let result = self.check_result.clone();
            Box::pin(async move { result })
        }

        fn download<'a>(
            &'a self,
            _version: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<std::path::PathBuf, String>> + Send + 'a>> {
            self.download_calls.fetch_add(1, Ordering::SeqCst);
            let result = self.download_result.clone();
            Box::pin(async move { result })
        }
    }

    fn ack_for(instruction: u8) -> String {
        let body = format!("0000{instruction:02x}");
        let lrc = control::lrc(body.as_bytes());
        format!("ab{body}{lrc:02x}")
    }

    fn tag_message_format_response(format: &TagMessageFormat) -> String {
        let mut data = vec![
            format.field_mask,
            format.id_byte_mask,
            format.ascii_header_1,
            format.ascii_header_2,
            format.binary_header_1,
            format.binary_header_2,
            format.trailer_1,
            format.trailer_2,
        ];
        if let Some(separator) = format.separator {
            data.push(separator);
        }

        let mut body = format!("00{:02x}11", data.len());
        for byte in data {
            body.push_str(&format!("{byte:02x}"));
        }
        let lrc = control::lrc(body.as_bytes());
        format!("ab{body}{lrc:02x}")
    }

    fn config3_response(mode: control::ReadMode, timeout: u8) -> String {
        let body = format!("000209{:02x}{timeout:02x}", mode.config3_value());
        let lrc = control::lrc(body.as_bytes());
        format!("ab{body}{lrc:02x}")
    }

    fn temp_token_file() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("create temp token dir");
        let path = dir.path().join("fake-token");
        std::fs::write(&path, "test-token\n")
            .unwrap_or_else(|e| panic!("write token {}: {e}", path.display()));
        let token_path = path.display().to_string().replace('\\', "/");
        (dir, token_path)
    }

    #[tokio::test]
    async fn update_apply_sets_failed_status_when_staged_file_missing() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .set_update_status(UpdateStatus::Downloaded {
                version: "1.2.3".to_owned(),
            })
            .await;
        let temp = tempfile::tempdir().expect("tempdir");
        server
            .set_staged_update_path(temp.path().join("missing-forwarder-staged"))
            .await;

        let addr = server.local_addr();
        let base = format!("http://{}", addr);
        let client = reqwest::Client::new();

        let apply_resp = client
            .post(format!("{}/update/apply", base))
            .send()
            .await
            .expect("POST /update/apply");
        assert_eq!(apply_resp.status(), 200);

        let mut saw_failed = false;
        let mut last_body = String::new();
        for _ in 0..20 {
            let resp = client
                .get(format!("{}/update/status", base))
                .send()
                .await
                .expect("GET /update/status");
            last_body = resp.text().await.expect("response body");
            if last_body.contains(r#""status":"failed""#) {
                saw_failed = true;
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }

        assert!(
            saw_failed,
            "status never became failed, last response: {last_body}"
        );
    }

    #[tokio::test]
    async fn status_json_returns_forwarder_state() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.set_forwarder_id("fwd-abc123").await;
        server
            .init_readers(&[("192.168.1.10".to_owned(), 10010)])
            .await;
        server
            .update_reader_state("192.168.1.10", ReaderConnectionState::Connected)
            .await;
        server
            .set_ups_status(UpsStatusState {
                available: false,
                status: Some(rt_domain::UpsStatus {
                    battery_percent: 42,
                    battery_voltage_mv: 3890,
                    charging: false,
                    power_plugged: false,
                    temperature_cdeg: 3010,
                    sampled_at: 1711929600000,
                }),
            })
            .await;

        let addr = server.local_addr();
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{}/api/v1/status", addr))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.expect("json body");
        assert_eq!(body["forwarder_id"], "fwd-abc123");
        assert_eq!(body["version"], "0.2.0");
        assert_eq!(body["ready"], true);
        assert_eq!(body["p2p_connected"], false);
        assert_eq!(body["restart_needed"], false);
        assert_eq!(body["ups_status"]["available"], false);
        assert_eq!(body["ups_status"]["status"]["battery_percent"], 42);
        assert_eq!(body["ups_status"]["status"]["power_plugged"], false);
        assert_eq!(body["readers"][0]["ip"], "192.168.1.10");
        assert_eq!(body["readers"][0]["state"], "connected");
    }

    #[tokio::test]
    async fn status_json_includes_fanout_dropped_total() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .store()
            .fanout_drop_counter()
            .fetch_add(3, Ordering::SeqCst);

        let addr = server.local_addr();
        let resp = reqwest::get(format!("http://{}/api/v1/status", addr))
            .await
            .expect("GET /api/v1/status");
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.expect("json body");
        assert_eq!(
            body["fanout_dropped_total"], 3,
            "status JSON must surface the fanout drop total"
        );
    }

    #[tokio::test]
    async fn status_json_serves_cached_server_status_without_outbound_io() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        // A "server" that accepts TCP connections but never responds: any
        // inline outbound HTTP call in the status handler would block on it
        // until the request timeout (~1s).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind hang listener");
        let hang_addr = listener.local_addr().expect("hang listener addr");
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => held.push(s),
                    Err(_) => break,
                }
            }
        });

        let mut config_file = NamedTempFile::new().expect("create temp config");
        let (_token_dir, token_path) = temp_token_file();
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
server_url = "http://{hang_addr}"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            Arc::new(Notify::new()),
        )
        .await
        .expect("start status server");
        server
            .store()
            .set_p2p_endpoint_id("endpoint-under-test".to_owned())
            .await;

        let client = reqwest::Client::new();
        let start = std::time::Instant::now();
        let resp = client
            .get(format!("http://{}/api/v1/status", server.local_addr()))
            .send()
            .await
            .expect("GET /api/v1/status");
        let elapsed = start.elapsed();
        assert_eq!(resp.status(), 200);
        assert!(
            elapsed < Duration::from_millis(500),
            "status endpoint must not perform outbound I/O inline (took {elapsed:?})"
        );

        let body: serde_json::Value = resp.json().await.expect("json body");
        let server_json = body.get("server").expect("server object");
        assert!(
            server_json.get("reachable").is_some(),
            "server.reachable missing: {server_json}"
        );
        assert_eq!(
            server_json["cached"], true,
            "server.cached must be true: {server_json}"
        );
        assert!(
            server_json.get("checked_unix_ms").is_some(),
            "server.checked_unix_ms missing: {server_json}"
        );
    }

    #[tokio::test]
    async fn refresh_reader_preserves_static_reader_info_fields() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;
        server
            .update_reader_info(
                reader_ip,
                crate::reader_control::ReaderInfo {
                    banner: Some("ARM9 Controller".to_owned()),
                    hardware: Some(crate::reader_control::HardwareInfo {
                        fw_version: "15.8".to_owned(),
                        hw_code: 0x8f,
                        reader_id: 0,
                        config3: 0,
                    }),
                    ..Default::default()
                },
            )
            .await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let feeder = tokio::spawn(async move {
            let ext_status_cmd = cmd_rx.recv().await.expect("ext status command");
            assert_eq!(
                std::str::from_utf8(&ext_status_cmd).expect("ext status command utf8"),
                "ab00ff4bc2\r\n"
            );
            assert!(
                control_sink
                    .feed(b"ab000d4b010b012f0000000059058f0c005a")
                    .await
            );

            let config3_cmd = cmd_rx.recv().await.expect("config3 command");
            assert_eq!(
                std::str::from_utf8(&config3_cmd).expect("config3 command utf8"),
                "ab00ff0995\r\n"
            );
            assert!(control_sink.feed(b"ab0002090305f3").await);

            let tag_format_cmd = cmd_rx.recv().await.expect("tag format command");
            let expected_tag_format = control::encode_command(&Command::GetTagMessageFormat, 0x00)
                .expect("encode tag format query");
            assert_eq!(tag_format_cmd, expected_tag_format);
            let tag_format = TagMessageFormat {
                field_mask: 0x7f,
                id_byte_mask: 0xfc,
                ascii_header_1: 0x61,
                ascii_header_2: 0x61,
                binary_header_1: 0xaa,
                binary_header_2: 0x00,
                trailer_1: 0x0d,
                trailer_2: 0x0a,
                separator: None,
            };
            assert!(
                control_sink
                    .feed(tag_message_format_response(&tag_format).as_bytes())
                    .await
            );

            let date_time_cmd = cmd_rx.recv().await.expect("date/time command");
            assert_eq!(
                std::str::from_utf8(&date_time_cmd).expect("date/time command utf8"),
                "ab00000222\r\n"
            );
            assert!(control_sink.feed(b"ab000902260306051855443727cf").await);
        });

        let client = reqwest::Client::new();
        let refresh = client
            .post(format!(
                "http://{}/api/v1/readers/{}/refresh",
                server.local_addr(),
                reader_ip
            ))
            .send()
            .await
            .expect("POST refresh");
        assert_eq!(refresh.status(), StatusCode::OK);

        feeder.await.expect("response feeder task");

        let status = client
            .get(format!("http://{}/api/v1/status", server.local_addr()))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(status.status(), StatusCode::OK);
        let body: serde_json::Value = status.json().await.expect("status json");

        let info = &body["readers"][0]["reader_info"];
        assert_eq!(info["hardware"]["fw_version"], "15.8");
        assert_eq!(info["banner"], "ARM9 Controller");
        assert_eq!(info["hardware"]["hw_code"], 143);
        assert_eq!(info["tto_enabled"], false);
    }

    #[tokio::test]
    async fn sync_clock_emits_reader_info_update_and_populates_missing_cache() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;

        let mut ui_rx = server.ui_sender().subscribe();

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let feeder = tokio::spawn(async move {
            let expected_date_time =
                control::encode_command(&Command::GetDateTime, 0x00).expect("encode get date/time");

            for _ in 0..3 {
                let get_cmd = cmd_rx.recv().await.expect("get date/time command");
                assert_eq!(get_cmd, expected_date_time);
                assert!(control_sink.feed(b"ab000902260306051855443727cf").await);
            }

            let set_cmd = cmd_rx.recv().await.expect("set date/time command");
            let set_text = std::str::from_utf8(&set_cmd).expect("set date/time utf8");
            assert!(
                set_text.starts_with("ab000701"),
                "unexpected set_date_time frame: {set_text}"
            );
            assert!(
                control_sink
                    .feed(ack_for(control::INSTR_SET_DATE_TIME).as_bytes())
                    .await
            );

            let verify_cmd = cmd_rx.recv().await.expect("verify date/time command");
            assert_eq!(verify_cmd, expected_date_time);
            assert!(control_sink.feed(b"ab000902260306051855443727cf").await);
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/readers/{}/sync-clock",
                server.local_addr(),
                reader_ip
            ))
            .send()
            .await
            .expect("POST sync-clock");
        assert_eq!(resp.status(), StatusCode::OK);

        feeder.await.expect("response feeder task");

        let event = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match ui_rx.recv().await.expect("ui event") {
                    crate::ui_events::ForwarderUiEvent::ReaderInfoUpdated { ip, info } => {
                        break (ip, info);
                    }
                    _ => continue,
                }
            }
        })
        .await
        .expect("reader info update event");
        assert_eq!(event.0, reader_ip);
        assert_eq!(
            event.1.clock.as_ref().expect("clock info").reader_clock,
            "2026-03-06T18:55:44.550"
        );

        let status = client
            .get(format!("http://{}/api/v1/status", server.local_addr()))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(status.status(), StatusCode::OK);

        let body: serde_json::Value = status.json().await.expect("status json");
        let info = &body["readers"][0]["reader_info"];
        assert_eq!(info["clock"]["reader_clock"], "2026-03-06T18:55:44.550");
        assert!(info["clock"]["drift_ms"].is_number());
    }

    #[tokio::test]
    async fn disconnect_ignores_late_reader_info_updates() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;
        server
            .update_reader_info(
                reader_ip,
                crate::reader_control::ReaderInfo {
                    banner: Some("connected-info".to_owned()),
                    ..Default::default()
                },
            )
            .await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Disconnected)
            .await;

        server
            .update_reader_info_unless_disconnected(
                reader_ip,
                crate::reader_control::ReaderInfo {
                    banner: Some("late-info".to_owned()),
                    ..Default::default()
                },
            )
            .await;

        let client = reqwest::Client::new();
        let status = client
            .get(format!("http://{}/api/v1/status", server.local_addr()))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(status.status(), StatusCode::OK);

        let body: serde_json::Value = status.json().await.expect("status json");
        let reader = &body["readers"][0];
        assert_eq!(reader["state"], "disconnected");
        assert!(reader["reader_info"].is_null());
    }

    #[tokio::test]
    async fn disconnect_ignores_late_cached_reader_info_updates() {
        let reader_ip = "192.168.1.11";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10011)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;
        server
            .update_reader_info(
                reader_ip,
                crate::reader_control::ReaderInfo {
                    banner: Some("connected-info".to_owned()),
                    ..Default::default()
                },
            )
            .await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Disconnected)
            .await;

        let state = AppState {
            store: server.store(),
            subsystem: server.subsystem_arc(),
            journal: Arc::new(Mutex::new(NoJournal)),
            version: Arc::new("0.2.0".to_owned()),
            config_state: None,
            restart_signal: None,
            logger: server.logger(),
            ui_tx: server.ui_sender(),
            status_event_tx: server.store.status_event_sender(),
            control_clients: server.control_clients().clone(),
            download_trackers: server.download_trackers().clone(),
            reconnect_notifies: server.reconnect_notifies().clone(),
        };
        update_cached_reader_info(
            &state,
            reader_ip,
            crate::reader_control::ReaderInfo {
                banner: Some("late-info".to_owned()),
                ..Default::default()
            },
        )
        .await;

        let client = reqwest::Client::new();
        let status = client
            .get(format!("http://{}/api/v1/status", server.local_addr()))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(status.status(), StatusCode::OK);

        let body: serde_json::Value = status.json().await.expect("status json");
        let reader = &body["readers"][0];
        assert_eq!(reader["state"], "disconnected");
        assert!(reader["reader_info"].is_null());
    }

    #[tokio::test]
    async fn sync_clock_succeeds_with_partial_rtt_probe_failure() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let feeder = tokio::spawn(async move {
            let expected_date_time =
                control::encode_command(&Command::GetDateTime, 0x00).expect("encode get date/time");
            let valid_response = b"ab000902260306051855443727cf";

            // Probe 1: valid response
            let cmd = cmd_rx.recv().await.expect("probe 1 command");
            assert_eq!(cmd, expected_date_time);
            assert!(control_sink.feed(valid_response).await);

            // Probe 2: malformed response → parse error consumed by pending request
            let cmd = cmd_rx.recv().await.expect("probe 2 command");
            assert_eq!(cmd, expected_date_time);
            assert!(control_sink.feed(b"ab0001").await);

            // Probe 3: valid response
            let cmd = cmd_rx.recv().await.expect("probe 3 command");
            assert_eq!(cmd, expected_date_time);
            assert!(control_sink.feed(valid_response).await);

            // SET_DATE_TIME
            let set_cmd = cmd_rx.recv().await.expect("set date/time command");
            let set_text = std::str::from_utf8(&set_cmd).expect("set date/time utf8");
            assert!(
                set_text.starts_with("ab000701"),
                "unexpected set_date_time frame: {set_text}"
            );
            assert!(
                control_sink
                    .feed(ack_for(control::INSTR_SET_DATE_TIME).as_bytes())
                    .await
            );

            // Verify GET_DATE_TIME
            let verify_cmd = cmd_rx.recv().await.expect("verify date/time command");
            assert_eq!(verify_cmd, expected_date_time);
            assert!(control_sink.feed(valid_response).await);
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/readers/{}/sync-clock",
                server.local_addr(),
                reader_ip
            ))
            .send()
            .await
            .expect("POST sync-clock");
        assert_eq!(resp.status(), StatusCode::OK);

        feeder.await.expect("response feeder task");
    }

    #[tokio::test]
    async fn sync_clock_returns_503_when_no_control_client() {
        let reader_ip = "192.168.1.99";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/readers/{}/sync-clock",
                server.local_addr(),
                reader_ip
            ))
            .send()
            .await
            .expect("POST sync-clock");
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_tto_returns_enabled_false_when_tag_format_bit_7_is_clear() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let current_format = TagMessageFormat {
            field_mask: 0x7f,
            id_byte_mask: 0xfc,
            ascii_header_1: 0x61,
            ascii_header_2: 0x61,
            binary_header_1: 0xaa,
            binary_header_2: 0x00,
            trailer_1: 0x0d,
            trailer_2: 0x0a,
            separator: None,
        };

        let feeder = tokio::spawn(async move {
            let query_cmd = cmd_rx.recv().await.expect("tag format query");
            let expected =
                control::encode_command(&Command::GetTagMessageFormat, 0x00).expect("encode query");
            assert_eq!(query_cmd, expected);
            assert!(
                control_sink
                    .feed(tag_message_format_response(&current_format).as_bytes())
                    .await
            );
        });

        let client = reqwest::Client::new();
        let resp = client
            .get(format!(
                "http://{}/api/v1/readers/{}/tto",
                server.local_addr(),
                reader_ip
            ))
            .send()
            .await
            .expect("GET tto");
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.expect("tto json");
        assert_eq!(body["enabled"], false);

        feeder.await.expect("response feeder task");
    }

    #[tokio::test]
    async fn put_tto_queries_current_format_rewrites_bit_7_and_returns_new_state() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let current_format = TagMessageFormat {
            field_mask: 0x7f,
            id_byte_mask: 0xfc,
            ascii_header_1: 0x61,
            ascii_header_2: 0x61,
            binary_header_1: 0xaa,
            binary_header_2: 0x00,
            trailer_1: 0x0d,
            trailer_2: 0x0a,
            separator: None,
        };
        let updated_format = current_format.with_tto_enabled(true);

        let feeder = tokio::spawn(async move {
            let first_query = cmd_rx.recv().await.expect("first tag format query");
            let expected_query =
                control::encode_command(&Command::GetTagMessageFormat, 0x00).expect("encode query");
            assert_eq!(first_query, expected_query);
            assert!(
                control_sink
                    .feed(tag_message_format_response(&current_format).as_bytes())
                    .await
            );

            let set_cmd = cmd_rx.recv().await.expect("set tag format");
            let expected_set = control::encode_command(
                &Command::SetTagMessageFormat {
                    format: updated_format.clone(),
                },
                0x00,
            )
            .expect("encode set");
            assert_eq!(set_cmd, expected_set);
            assert!(
                control_sink
                    .feed(ack_for(control::INSTR_TAG_MESSAGE_FORMAT).as_bytes())
                    .await
            );

            let second_query = cmd_rx.recv().await.expect("second tag format query");
            assert_eq!(second_query, expected_query);
            assert!(
                control_sink
                    .feed(tag_message_format_response(&updated_format).as_bytes())
                    .await
            );
        });

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "http://{}/api/v1/readers/{}/tto",
                server.local_addr(),
                reader_ip
            ))
            .header("content-type", "application/json")
            .body(r#"{"enabled":true}"#)
            .send()
            .await
            .expect("PUT tto");
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.expect("tto json");
        assert_eq!(body["enabled"], true);

        feeder.await.expect("response feeder task");
    }

    #[tokio::test]
    async fn put_tto_preserves_existing_tag_format_fields() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let current_format = TagMessageFormat {
            field_mask: 0x13,
            id_byte_mask: 0xa5,
            ascii_header_1: 0x23,
            ascii_header_2: 0x24,
            binary_header_1: 0xbb,
            binary_header_2: 0x01,
            trailer_1: 0x0a,
            trailer_2: 0x0d,
            separator: Some(0x2c),
        };
        let updated_format = current_format.with_tto_enabled(true);

        let feeder = tokio::spawn(async move {
            let expected_query =
                control::encode_command(&Command::GetTagMessageFormat, 0x00).expect("encode query");
            let first_query = cmd_rx.recv().await.expect("first query");
            assert_eq!(first_query, expected_query);
            assert!(
                control_sink
                    .feed(tag_message_format_response(&current_format).as_bytes())
                    .await
            );

            let set_cmd = cmd_rx.recv().await.expect("set tag format");
            let expected_set = control::encode_command(
                &Command::SetTagMessageFormat {
                    format: updated_format.clone(),
                },
                0x00,
            )
            .expect("encode set");
            assert_eq!(set_cmd, expected_set);
            assert_ne!(updated_format.field_mask, current_format.field_mask);
            assert_eq!(updated_format.id_byte_mask, current_format.id_byte_mask);
            assert_eq!(updated_format.ascii_header_1, current_format.ascii_header_1);
            assert_eq!(updated_format.ascii_header_2, current_format.ascii_header_2);
            assert_eq!(
                updated_format.binary_header_1,
                current_format.binary_header_1
            );
            assert_eq!(
                updated_format.binary_header_2,
                current_format.binary_header_2
            );
            assert_eq!(updated_format.trailer_1, current_format.trailer_1);
            assert_eq!(updated_format.trailer_2, current_format.trailer_2);
            assert_eq!(updated_format.separator, current_format.separator);
            assert!(
                control_sink
                    .feed(ack_for(control::INSTR_TAG_MESSAGE_FORMAT).as_bytes())
                    .await
            );

            let second_query = cmd_rx.recv().await.expect("second query");
            assert_eq!(second_query, expected_query);
            assert!(
                control_sink
                    .feed(tag_message_format_response(&updated_format).as_bytes())
                    .await
            );
        });

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "http://{}/api/v1/readers/{}/tto",
                server.local_addr(),
                reader_ip
            ))
            .header("content-type", "application/json")
            .body(r#"{"enabled":true}"#)
            .send()
            .await
            .expect("PUT tto");
        assert_eq!(resp.status(), StatusCode::OK);

        feeder.await.expect("response feeder task");
    }

    #[tokio::test]
    async fn set_read_mode_clears_clock_when_follow_up_poll_fails() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;
        server
            .update_reader_info(
                reader_ip,
                crate::reader_control::ReaderInfo {
                    config: Some(crate::reader_control::Config3Info {
                        mode: control::ReadMode::Raw,
                        timeout: 5,
                    }),
                    tto_enabled: Some(true),
                    clock: Some(crate::reader_control::ClockInfo {
                        reader_clock: "2026-03-06T18:55:44.000".to_owned(),
                        drift_ms: 123,
                    }),
                    ..Default::default()
                },
            )
            .await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let feeder = tokio::spawn(async move {
            let set_cmd = cmd_rx.recv().await.expect("set config3 command");
            let expected_set = control::encode_command(
                &Command::SetConfig3 {
                    mode: control::ReadMode::Event,
                    timeout: 7,
                },
                0x00,
            )
            .expect("encode set config3");
            assert_eq!(set_cmd, expected_set);
            assert!(
                control_sink
                    .feed(ack_for(control::INSTR_CONFIG3).as_bytes())
                    .await
            );

            let ext_status_cmd = cmd_rx.recv().await.expect("ext status command");
            assert_eq!(
                std::str::from_utf8(&ext_status_cmd).expect("ext status command utf8"),
                "ab00ff4bc2\r\n"
            );
            assert!(
                control_sink
                    .feed(b"ab000d4b010b012f0000000059058f0c005a")
                    .await
            );

            let config3_cmd = cmd_rx.recv().await.expect("config3 command");
            let expected_get =
                control::encode_command(&Command::GetConfig3, 0x00).expect("encode get config3");
            assert_eq!(config3_cmd, expected_get);
            assert!(
                control_sink
                    .feed(config3_response(control::ReadMode::Event, 7).as_bytes())
                    .await
            );

            let tag_format_cmd = cmd_rx.recv().await.expect("tag format command");
            let expected_tag_format = control::encode_command(&Command::GetTagMessageFormat, 0x00)
                .expect("encode tag format query");
            assert_eq!(tag_format_cmd, expected_tag_format);
            let tag_format = TagMessageFormat {
                field_mask: 0xff,
                id_byte_mask: 0xfc,
                ascii_header_1: 0x61,
                ascii_header_2: 0x61,
                binary_header_1: 0xaa,
                binary_header_2: 0x00,
                trailer_1: 0x0d,
                trailer_2: 0x0a,
                separator: None,
            };
            assert!(
                control_sink
                    .feed(tag_message_format_response(&tag_format).as_bytes())
                    .await
            );

            let date_time_cmd = cmd_rx.recv().await.expect("date/time command");
            let expected_date_time =
                control::encode_command(&Command::GetDateTime, 0x00).expect("encode get date/time");
            assert_eq!(date_time_cmd, expected_date_time);
            assert!(
                control_sink
                    .feed(ack_for(control::INSTR_GET_DATE_TIME).as_bytes())
                    .await
            );
        });

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "http://{}/api/v1/readers/{}/read-mode",
                server.local_addr(),
                reader_ip
            ))
            .header("content-type", "application/json")
            .body(r#"{"mode":"event","timeout":7}"#)
            .send()
            .await
            .expect("PUT read-mode");
        assert_eq!(resp.status(), StatusCode::OK);

        feeder.await.expect("response feeder task");

        let status = client
            .get(format!("http://{}/api/v1/status", server.local_addr()))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(status.status(), StatusCode::OK);

        let body: serde_json::Value = status.json().await.expect("status json");
        let info = &body["readers"][0]["reader_info"];
        assert_eq!(info["config"]["mode"], "event");
        assert_eq!(info["config"]["timeout"], 7);
        assert!(info["clock"].is_null());
    }

    #[tokio::test]
    async fn set_read_mode_uses_requested_config_when_follow_up_config_poll_fails() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;
        server
            .update_reader_info(
                reader_ip,
                crate::reader_control::ReaderInfo {
                    config: Some(crate::reader_control::Config3Info {
                        mode: control::ReadMode::Raw,
                        timeout: 5,
                    }),
                    ..Default::default()
                },
            )
            .await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let feeder = tokio::spawn(async move {
            let set_cmd = cmd_rx.recv().await.expect("set config3 command");
            let expected_set = control::encode_command(
                &Command::SetConfig3 {
                    mode: control::ReadMode::Event,
                    timeout: 7,
                },
                0x00,
            )
            .expect("encode set config3");
            assert_eq!(set_cmd, expected_set);
            assert!(
                control_sink
                    .feed(ack_for(control::INSTR_CONFIG3).as_bytes())
                    .await
            );

            let ext_status_cmd = cmd_rx.recv().await.expect("ext status command");
            assert_eq!(
                std::str::from_utf8(&ext_status_cmd).expect("ext status command utf8"),
                "ab00ff4bc2\r\n"
            );
            assert!(
                control_sink
                    .feed(b"ab000d4b010b012f0000000059058f0c005a")
                    .await
            );

            let config3_cmd = cmd_rx.recv().await.expect("config3 command");
            let expected_get =
                control::encode_command(&Command::GetConfig3, 0x00).expect("encode get config3");
            assert_eq!(config3_cmd, expected_get);
            assert!(control_sink.feed(b"not-a-config3-frame").await);

            let tag_format_cmd = cmd_rx.recv().await.expect("tag format command");
            let expected_tag_format = control::encode_command(&Command::GetTagMessageFormat, 0x00)
                .expect("encode tag format query");
            assert_eq!(tag_format_cmd, expected_tag_format);
            let tag_format = TagMessageFormat {
                field_mask: 0xff,
                id_byte_mask: 0xfc,
                ascii_header_1: 0x61,
                ascii_header_2: 0x61,
                binary_header_1: 0xaa,
                binary_header_2: 0x00,
                trailer_1: 0x0d,
                trailer_2: 0x0a,
                separator: None,
            };
            assert!(
                control_sink
                    .feed(tag_message_format_response(&tag_format).as_bytes())
                    .await
            );

            let date_time_cmd = cmd_rx.recv().await.expect("date/time command");
            let expected_date_time =
                control::encode_command(&Command::GetDateTime, 0x00).expect("encode get date/time");
            assert_eq!(date_time_cmd, expected_date_time);
            assert!(control_sink.feed(b"ab000902260306051855443727cf").await);
        });

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "http://{}/api/v1/readers/{}/read-mode",
                server.local_addr(),
                reader_ip
            ))
            .header("content-type", "application/json")
            .body(r#"{"mode":"event","timeout":7}"#)
            .send()
            .await
            .expect("PUT read-mode");
        assert_eq!(resp.status(), StatusCode::OK);

        feeder.await.expect("response feeder task");

        let status = client
            .get(format!("http://{}/api/v1/status", server.local_addr()))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(status.status(), StatusCode::OK);

        let body: serde_json::Value = status.json().await.expect("status json");
        let info = &body["readers"][0]["reader_info"];
        assert_eq!(info["config"]["mode"], "event");
        assert_eq!(info["config"]["timeout"], 7);
    }

    #[tokio::test]
    async fn set_read_mode_updates_cached_config_when_follow_up_poll_succeeds() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;
        server
            .update_reader_info(
                reader_ip,
                crate::reader_control::ReaderInfo {
                    config: Some(crate::reader_control::Config3Info {
                        mode: control::ReadMode::Raw,
                        timeout: 5,
                    }),
                    ..Default::default()
                },
            )
            .await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let feeder = tokio::spawn(async move {
            let set_cmd = cmd_rx.recv().await.expect("set config3 command");
            let expected_set = control::encode_command(
                &Command::SetConfig3 {
                    mode: control::ReadMode::FirstLastSeen,
                    timeout: 9,
                },
                0x00,
            )
            .expect("encode set config3");
            assert_eq!(set_cmd, expected_set);
            assert!(
                control_sink
                    .feed(ack_for(control::INSTR_CONFIG3).as_bytes())
                    .await
            );

            let ext_status_cmd = cmd_rx.recv().await.expect("ext status command");
            assert_eq!(
                std::str::from_utf8(&ext_status_cmd).expect("ext status command utf8"),
                "ab00ff4bc2\r\n"
            );
            assert!(
                control_sink
                    .feed(b"ab000d4b010b012f0000000059058f0c005a")
                    .await
            );

            let config3_cmd = cmd_rx.recv().await.expect("config3 command");
            let expected_get =
                control::encode_command(&Command::GetConfig3, 0x00).expect("encode get config3");
            assert_eq!(config3_cmd, expected_get);
            assert!(
                control_sink
                    .feed(config3_response(control::ReadMode::FirstLastSeen, 9).as_bytes())
                    .await
            );

            let tag_format_cmd = cmd_rx.recv().await.expect("tag format command");
            let expected_tag_format = control::encode_command(&Command::GetTagMessageFormat, 0x00)
                .expect("encode tag format query");
            assert_eq!(tag_format_cmd, expected_tag_format);
            let tag_format = TagMessageFormat {
                field_mask: 0x7f,
                id_byte_mask: 0xfc,
                ascii_header_1: 0x61,
                ascii_header_2: 0x61,
                binary_header_1: 0xaa,
                binary_header_2: 0x00,
                trailer_1: 0x0d,
                trailer_2: 0x0a,
                separator: None,
            };
            assert!(
                control_sink
                    .feed(tag_message_format_response(&tag_format).as_bytes())
                    .await
            );

            let date_time_cmd = cmd_rx.recv().await.expect("date/time command");
            let expected_date_time =
                control::encode_command(&Command::GetDateTime, 0x00).expect("encode get date/time");
            assert_eq!(date_time_cmd, expected_date_time);
            assert!(control_sink.feed(b"ab000902260306051855443727cf").await);
        });

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "http://{}/api/v1/readers/{}/read-mode",
                server.local_addr(),
                reader_ip
            ))
            .header("content-type", "application/json")
            .body(r#"{"mode":"fsls","timeout":9}"#)
            .send()
            .await
            .expect("PUT read-mode");
        assert_eq!(resp.status(), StatusCode::OK);

        feeder.await.expect("response feeder task");

        let status = client
            .get(format!("http://{}/api/v1/status", server.local_addr()))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(status.status(), StatusCode::OK);

        let body: serde_json::Value = status.json().await.expect("status json");
        let info = &body["readers"][0]["reader_info"];
        assert_eq!(info["config"]["mode"], "fsls");
        assert_eq!(info["config"]["timeout"], 9);
        assert_eq!(info["tto_enabled"], false);
    }

    #[tokio::test]
    async fn set_read_mode_invalid_mode_returns_valid_json_error() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;

        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1);
        let (control_client, _control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "http://{}/api/v1/readers/{}/read-mode",
                server.local_addr(),
                reader_ip
            ))
            .header("content-type", "application/json")
            .body(r#"{"mode":"bad\"mode"}"#)
            .send()
            .await
            .expect("PUT read-mode");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let body: serde_json::Value = resp.json().await.expect("json error response");
        assert_eq!(body["error"], "unknown mode: bad\"mode");
    }

    #[tokio::test]
    async fn set_recording_returns_true_even_if_follow_up_poll_fails() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;
        server
            .update_reader_info(
                reader_ip,
                crate::reader_control::ReaderInfo {
                    recording: Some(false),
                    estimated_stored_reads: Some(0),
                    ..Default::default()
                },
            )
            .await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server.register_control_client(reader_ip, Arc::new(control_client));

        let feeder = tokio::spawn(async move {
            let set_cmd = cmd_rx.recv().await.expect("set recording command");
            let expected = control::encode_command(&Command::SetRecordingState { on: true }, 0x00)
                .expect("encode set recording");
            assert_eq!(set_cmd, expected);
            assert!(
                control_sink
                    .feed(b"ab000c4b010000000000000059058f015c")
                    .await
            );

            for _ in 0..4 {
                let _ = cmd_rx.recv().await.expect("follow-up poll command");
                assert!(control_sink.feed(b"zz not-a-frame").await);
            }
        });

        let resp = reqwest::Client::new()
            .put(format!(
                "http://{}/api/v1/readers/{}/recording",
                server.local_addr(),
                reader_ip
            ))
            .header("content-type", "application/json")
            .body(r#"{"enabled":true}"#)
            .send()
            .await
            .expect("PUT recording");
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.expect("json body");
        assert_eq!(body["recording"], true);

        // Verify cached reader_info also reflects the set_recording response
        let status_resp = reqwest::Client::new()
            .get(format!("http://{}/api/v1/status", server.local_addr()))
            .send()
            .await
            .expect("GET /api/v1/status");
        let status_body: serde_json::Value = status_resp.json().await.expect("status json");
        let cached_info = &status_body["readers"][0]["reader_info"];
        assert_eq!(
            cached_info["recording"], true,
            "cached reader_info.recording should reflect set_recording response"
        );
        assert!(
            cached_info["estimated_stored_reads"].is_number(),
            "cached reader_info.estimated_stored_reads should be present from set_recording response"
        );

        feeder.await.expect("feeder task");
    }

    #[tokio::test]
    async fn sync_clock_broadcasts_reader_info_updated() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;
        server
            .update_reader_info(reader_ip, crate::reader_control::ReaderInfo::default())
            .await;

        let mut rx = server.ui_sender().subscribe();
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server.register_control_client(reader_ip, Arc::new(control_client));

        let feeder = tokio::spawn(async move {
            for _ in 0..3 {
                let _ = cmd_rx.recv().await.expect("clock sync read command");
                assert!(control_sink.feed(b"ab000902260306051855443727cf").await);
            }
            let _ = cmd_rx.recv().await.expect("clock sync set command");
            assert!(
                control_sink
                    .feed(ack_for(control::INSTR_SET_DATE_TIME).as_bytes())
                    .await
            );
            let _ = cmd_rx.recv().await.expect("clock sync verify command");
            assert!(control_sink.feed(b"ab000902260306051855443727cf").await);
        });

        let resp = reqwest::Client::new()
            .post(format!(
                "http://{}/api/v1/readers/{}/sync-clock",
                server.local_addr(),
                reader_ip
            ))
            .send()
            .await
            .expect("POST sync-clock");
        assert_eq!(resp.status(), StatusCode::OK);

        let (ip, info) = tokio::time::timeout(std::time::Duration::from_secs(1), async move {
            loop {
                match rx.recv().await.expect("recv event") {
                    crate::ui_events::ForwarderUiEvent::ReaderInfoUpdated { ip, info } => {
                        break (ip, info);
                    }
                    _ => continue,
                }
            }
        })
        .await
        .expect("event timeout");

        assert_eq!(ip, reader_ip);
        assert!(info.clock.is_some());

        feeder.await.expect("feeder task");
    }

    #[tokio::test]
    async fn set_ready_broadcasts_status_changed() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::not_ready("starting".to_owned()),
        )
        .await
        .expect("start status server");

        let mut rx = server.ui_sender().subscribe();
        server.set_ready().await;

        let evt = tokio::time::timeout(Duration::from_millis(250), rx.recv())
            .await
            .expect("event timeout")
            .expect("recv event");
        match evt {
            crate::ui_events::ForwarderUiEvent::StatusChanged { ready, .. } => {
                assert!(ready);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_update_status_broadcasts_update_status_changed() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let mut rx = server.ui_sender().subscribe();
        server
            .set_update_status(UpdateStatus::Downloaded {
                version: "1.2.3".to_owned(),
            })
            .await;

        let evt = tokio::time::timeout(Duration::from_millis(250), rx.recv())
            .await
            .expect("event timeout")
            .expect("recv event");
        match evt {
            crate::ui_events::ForwarderUiEvent::UpdateStatusChanged { status } => match status {
                UpdateStatus::Downloaded { version } => {
                    assert_eq!(version, "1.2.3");
                }
                other => panic!("unexpected status: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn config_save_broadcasts_status_changed_restart_needed() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        let (_token_dir, token_path) = temp_token_file();
        write!(
            config_file,
            r#"schema_version = 1
display_name = "Start Line"
[p2p]
server_url = "https://timing.example.com"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        let mut rx = server.ui_sender().subscribe();
        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/config/general",
                server.local_addr()
            ))
            .header("content-type", "application/json")
            .body(r#"{"display_name":"Updated"}"#)
            .send()
            .await
            .expect("post config");
        assert_eq!(resp.status(), StatusCode::OK);

        let evt = tokio::time::timeout(Duration::from_millis(250), rx.recv())
            .await
            .expect("event timeout")
            .expect("recv event");
        match evt {
            crate::ui_events::ForwarderUiEvent::StatusChanged { restart_needed, .. } => {
                assert!(restart_needed);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn config_update_endpoint_updates_runtime_mode_and_sets_restart_needed() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        let (_token_dir, token_path) = temp_token_file();
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
server_url = "https://timing.example.com"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        assert_eq!(
            server.subsystem_arc().lock().await.update_mode,
            rt_updater::UpdateMode::CheckAndDownload
        );

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/config/update",
                server.local_addr()
            ))
            .header("content-type", "application/json")
            .body(r#"{"mode":"check-only"}"#)
            .send()
            .await
            .expect("post config update");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            server.subsystem_arc().lock().await.update_mode,
            rt_updater::UpdateMode::CheckOnly
        );
        assert!(
            server.restart_needed().await,
            "restart_needed must be true after update config change"
        );
    }

    #[tokio::test]
    async fn config_update_endpoint_rejects_invalid_mode() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        let (_token_dir, token_path) = temp_token_file();
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
server_url = "https://timing.example.com"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/config/update",
                server.local_addr()
            ))
            .header("content-type", "application/json")
            .body(r#"{"mode":"bogus"}"#)
            .send()
            .await
            .expect("post config update");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            server.subsystem_arc().lock().await.update_mode,
            rt_updater::UpdateMode::CheckAndDownload
        );
    }

    #[tokio::test]
    async fn config_p2p_section_rejects_cross_field_loader_validation_failure() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let token_file = NamedTempFile::new().expect("create temp token");
        let token_path = token_file.path().display().to_string().replace('\\', "/");
        std::fs::write(token_file.path(), "test-token\n").expect("write temp token");

        let mut config_file = NamedTempFile::new().expect("create temp config");
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
enabled = false
max_concurrent_bidi_streams = 1
server_url = "https://timing.example.com"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{}/api/v1/config/p2p", server.local_addr()))
            .header("content-type", "application/json")
            .body(r#"{"enabled":true}"#)
            .send()
            .await
            .expect("post p2p config update");

        let status = resp.status();
        let body = resp.text().await.expect("read response body");
        assert_eq!(status, StatusCode::BAD_REQUEST, "response body: {body}");
        assert!(
            body.contains("config validation failed")
                && body.contains("max_concurrent_bidi_streams"),
            "unexpected response body: {body}"
        );

        let loaded = crate::config::load_config_from_path(config_file.path()).expect("load config");
        assert!(!loaded.p2p.enabled);
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[tokio::test]
    async fn config_screen_endpoint_updates_screen_and_sets_restart_needed() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        let (_token_dir, token_path) = temp_token_file();
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
server_url = "https://timing.example.com"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/config/screen",
                server.local_addr()
            ))
            .header("content-type", "application/json")
            .body(r#"{"backend":"lcd","lcd":{"rotation":"portrait"}}"#)
            .send()
            .await
            .expect("post config screen");
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            server.restart_needed().await,
            "restart_needed must be true after screen config change"
        );

        let updated = std::fs::read_to_string(config_file.path()).expect("read config file");
        assert!(updated.contains("[screen]"), "updated config: {updated}");
        assert!(
            updated.contains("backend = \"lcd\""),
            "updated config: {updated}"
        );
    }

    // The screen config endpoint must reject values the loader would reject, so a
    // 200-OK change can't make the forwarder fail to boot on the requested restart.
    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[tokio::test]
    async fn config_screen_endpoint_rejects_invalid_lcd_config() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        let (_token_dir, token_path) = temp_token_file();
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
server_url = "https://timing.example.com"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");
        let original = std::fs::read_to_string(config_file.path()).expect("read config file");

        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            Arc::new(Notify::new()),
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        // Landscape is a valid enum value but unsupported by the portrait renderer.
        let resp = client
            .post(format!(
                "http://{}/api/v1/config/screen",
                server.local_addr()
            ))
            .header("content-type", "application/json")
            .body(r#"{"backend":"lcd","lcd":{"rotation":"landscape"}}"#)
            .send()
            .await
            .expect("post config screen");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(
            !server.restart_needed().await,
            "restart must not be flagged for a rejected config"
        );
        let after = std::fs::read_to_string(config_file.path()).expect("read config file");
        assert_eq!(after, original, "rejected config must not be persisted");
    }

    #[tokio::test]
    async fn update_check_skips_download_in_check_only_mode() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");
        let download_calls = Arc::new(AtomicUsize::new(0));
        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::Available {
                version: "1.2.3".to_owned(),
            }),
            download_result: Ok(std::path::PathBuf::from("/tmp/unused")),
            download_calls: Arc::clone(&download_calls),
        };

        let workflow_state = server.store().workflow_state();
        let status = run_check(&workflow_state, &checker, rt_updater::UpdateMode::CheckOnly).await;

        assert_eq!(
            status,
            UpdateStatus::Available {
                version: "1.2.3".to_owned()
            }
        );
        assert_eq!(download_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn update_check_skips_download_in_disabled_mode() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");
        let download_calls = Arc::new(AtomicUsize::new(0));
        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::Available {
                version: "1.2.3".to_owned(),
            }),
            download_result: Ok(std::path::PathBuf::from("/tmp/unused")),
            download_calls: Arc::clone(&download_calls),
        };

        let workflow_state = server.store().workflow_state();
        let status = run_check(&workflow_state, &checker, rt_updater::UpdateMode::Disabled).await;

        assert_eq!(
            status,
            UpdateStatus::Available {
                version: "1.2.3".to_owned()
            }
        );
        assert_eq!(download_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn update_check_downloads_in_check_and_download_mode() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");
        let download_calls = Arc::new(AtomicUsize::new(0));
        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::Available {
                version: "1.2.3".to_owned(),
            }),
            download_result: Ok(std::path::PathBuf::from("/tmp/staged-forwarder")),
            download_calls: Arc::clone(&download_calls),
        };

        let workflow_state = server.store().workflow_state();
        let status = run_check(
            &workflow_state,
            &checker,
            rt_updater::UpdateMode::CheckAndDownload,
        )
        .await;

        assert_eq!(
            status,
            UpdateStatus::Downloaded {
                version: "1.2.3".to_owned()
            }
        );
        assert_eq!(download_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            server.subsystem_arc().lock().await.staged_update_path,
            Some(std::path::PathBuf::from("/tmp/staged-forwarder"))
        );
    }

    #[test]
    fn apply_via_restart_env_parsing() {
        assert!(apply_via_restart_from_env(Some("1".to_owned())));
        assert!(apply_via_restart_from_env(Some("true".to_owned())));
        assert!(apply_via_restart_from_env(Some("YES".to_owned())));
        assert!(apply_via_restart_from_env(Some(" on ".to_owned())));
        assert!(!apply_via_restart_from_env(None));
        assert!(!apply_via_restart_from_env(Some("0".to_owned())));
        assert!(!apply_via_restart_from_env(Some("false".to_owned())));
    }

    #[tokio::test]
    async fn update_download_downloads_when_available() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .set_update_status(UpdateStatus::Available {
                version: "2.0.0".to_owned(),
            })
            .await;

        let download_calls = Arc::new(AtomicUsize::new(0));
        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(std::path::PathBuf::from("/tmp/staged-forwarder")),
            download_calls: Arc::clone(&download_calls),
        };

        let workflow_state = server.store().workflow_state();
        let status = run_download(&workflow_state, &checker).await;

        assert_eq!(
            status,
            Ok(UpdateStatus::Downloaded {
                version: "2.0.0".to_owned()
            })
        );
        assert_eq!(download_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            server.subsystem_arc().lock().await.staged_update_path,
            Some(std::path::PathBuf::from("/tmp/staged-forwarder"))
        );
    }

    #[tokio::test]
    async fn update_download_failure_emits_failed_event() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .set_update_status(UpdateStatus::Available {
                version: "2.0.0".to_owned(),
            })
            .await;
        let mut rx = server.ui_sender().subscribe();

        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Err("boom".to_owned()),
            download_calls: Arc::new(AtomicUsize::new(0)),
        };

        let workflow_state = server.store().workflow_state();
        let status = run_download(&workflow_state, &checker).await;
        assert_eq!(
            status,
            Err(UpdateStatus::Failed {
                error: "boom".to_owned()
            })
        );

        let evt = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
            .await
            .expect("timed out waiting for ui event")
            .expect("recv event");
        match evt {
            crate::ui_events::ForwarderUiEvent::UpdateStatusChanged { status } => match status {
                UpdateStatus::Failed { error } => assert_eq!(error, "boom"),
                other => panic!("unexpected status: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn update_download_returns_conflict_when_up_to_date() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(std::path::PathBuf::from("/tmp/unused")),
            download_calls: Arc::new(AtomicUsize::new(0)),
        };

        let workflow_state = server.store().workflow_state();
        let status = run_download(&workflow_state, &checker).await;
        assert!(status.is_err());
    }

    #[tokio::test]
    async fn update_download_is_idempotent_when_already_downloaded() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .set_update_status(UpdateStatus::Downloaded {
                version: "2.0.0".to_owned(),
            })
            .await;

        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(std::path::PathBuf::from("/tmp/unused")),
            download_calls: Arc::new(AtomicUsize::new(0)),
        };

        let workflow_state = server.store().workflow_state();
        let status = run_download(&workflow_state, &checker).await;
        assert_eq!(
            status,
            Ok(UpdateStatus::Downloaded {
                version: "2.0.0".to_owned()
            })
        );
    }

    #[tokio::test]
    async fn set_reader_epoch_name_broadcasts_reader_updated_with_name() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .init_readers(&[("192.168.1.10".to_owned(), 10010)])
            .await;
        server
            .update_reader_state("192.168.1.10", ReaderConnectionState::Connected)
            .await;

        let mut rx = server.ui_sender().subscribe();

        // Set epoch name
        server
            .set_reader_epoch_name("192.168.1.10", Some("Race Day".to_owned()))
            .await;

        let evt = tokio::time::timeout(Duration::from_millis(250), rx.recv())
            .await
            .expect("event timeout")
            .expect("recv event");
        match evt {
            crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                ip,
                current_epoch_name,
                ..
            } => {
                assert_eq!(ip, "192.168.1.10");
                assert_eq!(current_epoch_name, Some("Race Day".to_owned()));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_reader_epoch_name_to_none_clears_name() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .init_readers(&[("192.168.1.10".to_owned(), 10010)])
            .await;
        server
            .set_reader_epoch_name("192.168.1.10", Some("Race Day".to_owned()))
            .await;

        let mut rx = server.ui_sender().subscribe();

        // Clear epoch name
        server.set_reader_epoch_name("192.168.1.10", None).await;

        let evt = tokio::time::timeout(Duration::from_millis(250), rx.recv())
            .await
            .expect("event timeout")
            .expect("recv event");
        match evt {
            crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                current_epoch_name, ..
            } => {
                assert_eq!(current_epoch_name, None);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn status_json_includes_current_epoch_id_and_created_time() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("journal.db");
        let mut journal = Journal::open(&path).expect("open journal");
        journal
            .ensure_stream_state("192.168.1.10", 3)
            .expect("ensure stream");
        let journal = Arc::new(Mutex::new(journal));

        let server = StatusServer::start_with_journal(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            journal,
        )
        .await
        .expect("start status server");

        server
            .init_readers(&[("192.168.1.10".to_owned(), 10010)])
            .await;

        let addr = server.local_addr();
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{}/api/v1/status", addr))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.expect("json body");
        assert_eq!(body["readers"][0]["current_epoch"], 3);
        assert!(body["readers"][0]["current_epoch_created_unix_ms"].is_number());
    }

    #[tokio::test]
    async fn status_json_includes_current_epoch_name() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .init_readers(&[("192.168.1.10".to_owned(), 10010)])
            .await;
        server
            .set_reader_epoch_name("192.168.1.10", Some("Race Day".to_owned()))
            .await;

        let addr = server.local_addr();
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{}/api/v1/status", addr))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.expect("json body");
        assert_eq!(body["readers"][0]["current_epoch_name"], "Race Day");
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[tokio::test]
    async fn display_state_includes_forwarder_name_and_cpu_temp() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.set_display_name(Some("Start Line".to_owned())).await;
        server.set_cpu_temp(Some(48.5)).await;

        let addr = server.local_addr();
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{}/api/v1/display-state", addr))
            .send()
            .await
            .expect("GET /api/v1/display-state");
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.expect("json body");
        assert_eq!(body["forwarder_name"], "Start Line");
        assert_eq!(body["cpu_temp_celsius"], 48.5);
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[tokio::test]
    async fn set_ready_with_display_sender_publishes_once() {
        let mut server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::not_ready("booting".to_owned()),
        )
        .await
        .expect("start status server");

        let (display_tx, mut display_rx) =
            tokio::sync::watch::channel(rt_screen::state::DisplayState::initial());
        server.set_display_sender(display_tx);

        let (seen_tx, mut seen_rx) = tokio::sync::mpsc::channel(4);
        let watcher = tokio::spawn(async move {
            while display_rx.changed().await.is_ok() {
                let _ = seen_tx.send(()).await;
            }
        });
        tokio::task::yield_now().await;

        tokio::time::timeout(Duration::from_millis(100), server.set_ready())
            .await
            .expect("set_ready timed out");
        tokio::time::timeout(Duration::from_millis(100), seen_rx.recv())
            .await
            .expect("display state publish timed out")
            .expect("display state sender dropped");
        tokio::time::timeout(Duration::from_millis(100), seen_rx.recv())
            .await
            .expect_err("display state should publish exactly once");
        watcher.abort();
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    #[tokio::test]
    async fn cpu_temp_cache_update_does_not_publish_display_state() {
        let mut server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let (display_tx, mut display_rx) =
            tokio::sync::watch::channel(rt_screen::state::DisplayState::initial());
        server.set_display_sender(display_tx);

        server.set_cpu_temp_cached(Some(41.0)).await;

        tokio::time::timeout(Duration::from_millis(100), display_rx.changed())
            .await
            .expect_err("cpu temp cache update should not publish display state");
    }

    #[tokio::test]
    async fn download_reads_returns_202_and_409_on_double_trigger() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;

        // Register a DownloadTracker for this reader
        let tracker = Arc::new(tokio::sync::Mutex::new(
            crate::reader_control::DownloadTracker::new(),
        ));
        server.register_download_tracker(reader_ip, Arc::clone(&tracker));

        // Register a ControlClient so the handler doesn't 404
        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        // Spawn a task to feed responses to the 3-step start_download sequence
        let feeder = tokio::spawn(async move {
            // Step 1: init (0x4b 0x02)
            let _cmd1 = cmd_rx.recv().await.expect("init command");
            control_sink
                .feed(b"ab000d4b010b012f0000000059058f0c005a")
                .await;
            // Step 2: configure (0x4b 0x07 0x01 0x05)
            let _cmd2 = cmd_rx.recv().await.expect("configure command");
            control_sink
                .feed(b"ab000d4b010b012f0000000059058f0c005a")
                .await;
            // Step 3: start (0x4b 0x01 0x01)
            let _cmd3 = cmd_rx.recv().await.expect("start command");
            control_sink
                .feed(b"ab000d4b010b012f0000000059058f0c005a")
                .await;
        });

        let client = reqwest::Client::new();
        let base = format!("http://{}", server.local_addr());

        // First POST should return 202 Accepted
        let resp1 = client
            .post(format!(
                "{}/api/v1/readers/{}/download-reads",
                base, reader_ip
            ))
            .send()
            .await
            .expect("POST download-reads");
        assert_eq!(resp1.status(), StatusCode::ACCEPTED);

        let body: serde_json::Value = resp1.json().await.expect("json body");
        assert_eq!(body["status"], "started");

        // Wait for the background task to move tracker to Downloading state
        feeder.await.expect("feeder task");
        // Give the background spawn a moment to update state
        sleep(Duration::from_millis(50)).await;

        // Second POST should return 409 Conflict
        let resp2 = client
            .post(format!(
                "{}/api/v1/readers/{}/download-reads",
                base, reader_ip
            ))
            .send()
            .await
            .expect("POST download-reads again");
        assert_eq!(resp2.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn download_reads_second_trigger_conflicts_even_before_startup_completes() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        let tracker = Arc::new(tokio::sync::Mutex::new(
            crate::reader_control::DownloadTracker::new(),
        ));
        server.register_download_tracker(reader_ip, Arc::clone(&tracker));

        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (control_client, _control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let client = reqwest::Client::new();
        let base = format!("http://{}", server.local_addr());

        let resp1 = client
            .post(format!(
                "{}/api/v1/readers/{}/download-reads",
                base, reader_ip
            ))
            .send()
            .await
            .expect("first POST download-reads");
        assert_eq!(resp1.status(), StatusCode::ACCEPTED);

        let resp2 = client
            .post(format!(
                "{}/api/v1/readers/{}/download-reads",
                base, reader_ip
            ))
            .send()
            .await
            .expect("second POST download-reads");
        assert_eq!(resp2.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn download_progress_does_not_emit_idle_immediately_after_start_trigger() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        let tracker = Arc::new(tokio::sync::Mutex::new(
            crate::reader_control::DownloadTracker::new(),
        ));
        server.register_download_tracker(reader_ip, Arc::clone(&tracker));

        let (cmd_tx, _cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let (control_client, _control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let client = reqwest::Client::new();
        let base = format!("http://{}", server.local_addr());

        let start_resp = client
            .post(format!(
                "{}/api/v1/readers/{}/download-reads",
                base, reader_ip
            ))
            .send()
            .await
            .expect("POST download-reads");
        assert_eq!(start_resp.status(), StatusCode::ACCEPTED);

        let progress_resp = client
            .get(format!(
                "{}/api/v1/readers/{}/download-reads/progress",
                base, reader_ip
            ))
            .send()
            .await
            .expect("GET progress SSE");
        assert_eq!(progress_resp.status(), StatusCode::OK);

        let first_body =
            tokio::time::timeout(Duration::from_millis(200), progress_resp.text()).await;
        if let Ok(Ok(text)) = first_body {
            assert!(
                !text.contains(r#""state":"idle""#),
                "progress stream must not terminate with idle immediately after start trigger; chunk={text:?}"
            );
        }
    }

    #[tokio::test]
    async fn download_progress_broadcasts_forwarder_status_event() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let tracker = Arc::new(tokio::sync::Mutex::new(
            crate::reader_control::DownloadTracker::new(),
        ));
        server.register_download_tracker(reader_ip, Arc::clone(&tracker));
        let mut events = server.status_feed().subscribe_and_snapshot().await.0;

        {
            let mut tracker = tracker.lock().await;
            tracker.begin_startup();
            tracker.start(100);
            tracker.record_read();
        }

        let event = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match events.recv().await.expect("status event") {
                    ForwarderStatusEvent::DownloadProgress { stream_id, event } => {
                        break (stream_id, event);
                    }
                    _ => continue,
                }
            }
        })
        .await
        .expect("download progress status event");

        assert_eq!(event.0, reader_ip);
        assert!(matches!(
            event.1,
            crate::reader_control::DownloadEvent::Downloading {
                reads_received: 1,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn record_read_marks_reader_dirty_without_p2p_broadcast() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");
        server
            .init_readers(&[("10.0.0.9:10000".to_owned(), 10_001)])
            .await;
        let (mut events, _snapshot) = server.status_feed().subscribe_and_snapshot().await;

        server.record_read("10.0.0.9:10000").await;
        server.record_read("10.0.0.9:10000").await;

        // Reads mark the reader dirty for the coalescing broadcaster rather
        // than pushing a per-read P2P status delta. (The dirty flag may already
        // have been consumed if the background broadcaster ticked, in which
        // case the delta must be on the feed instead.)
        let dirty = {
            let subsystem = server.subsystem_arc();
            let ss = subsystem.lock().await;
            ss.read_counts_dirty.contains("10.0.0.9:10000")
        };
        if !dirty {
            match events.try_recv() {
                Ok(ForwarderStatusEvent::ReaderStatus { stream_id, status }) => {
                    assert_eq!(stream_id, "10.0.0.9:10000");
                    assert_eq!(status.reads_since_restart, 2);
                }
                other => panic!(
                    "reader must be dirty or a coalesced delta must be on the feed, got {other:?}"
                ),
            }
        }
    }

    #[tokio::test]
    async fn broadcast_dirty_read_counts_coalesces_and_clears() {
        let subsystem = Arc::new(Mutex::new(SubsystemStatus::ready()));
        let (status_event_tx, mut events) = broadcast::channel(16);
        {
            let mut ss = subsystem.lock().await;
            ss.readers.insert(
                "10.0.0.9:10000".to_owned(),
                ReaderStatus {
                    state: ReaderConnectionState::Connected,
                    last_seen: Some(Instant::now()),
                    reads_since_restart: 2,
                    reads_total: 42,
                    local_port: 10_001,
                    current_epoch: None,
                    current_epoch_created_unix_ms: None,
                    current_epoch_name: None,
                    reader_info: None,
                },
            );
            ss.read_counts_dirty.insert("10.0.0.9:10000".to_owned());
            // A dirty entry with no matching reader must be skipped silently.
            ss.read_counts_dirty.insert("10.0.0.250:10000".to_owned());
        }

        broadcast_dirty_read_counts(&subsystem, &status_event_tx).await;

        match events.try_recv().expect("one coalesced ReaderStatus delta") {
            ForwarderStatusEvent::ReaderStatus { stream_id, status } => {
                assert_eq!(stream_id, "10.0.0.9:10000");
                assert_eq!(status.reads_since_restart, 2);
                assert_eq!(status.reads_total, 42);
            }
            other => panic!("expected ReaderStatus delta, got {other:?}"),
        }
        assert!(
            matches!(
                events.try_recv(),
                Err(broadcast::error::TryRecvError::Empty)
            ),
            "exactly one delta per dirty reader per tick"
        );
        assert!(
            subsystem.lock().await.read_counts_dirty.is_empty(),
            "dirty set must be drained by the broadcast"
        );

        // A second tick without new reads must broadcast nothing.
        broadcast_dirty_read_counts(&subsystem, &status_event_tx).await;
        assert!(matches!(
            events.try_recv(),
            Err(broadcast::error::TryRecvError::Empty)
        ));
    }

    #[tokio::test]
    async fn download_reads_returns_503_for_unknown_reader() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/readers/10.0.0.99/download-reads",
                server.local_addr()
            ))
            .send()
            .await
            .expect("POST download-reads unknown reader");
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn status_json_epoch_name_null_when_not_set() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .init_readers(&[("192.168.1.10".to_owned(), 10010)])
            .await;

        let addr = server.local_addr();
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{}/api/v1/status", addr))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.expect("json body");
        assert!(body["readers"][0]["current_epoch_name"].is_null());
    }

    #[tokio::test]
    async fn reconnect_endpoint_fires_notify() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .init_readers(&[("192.168.1.10".to_owned(), 10010)])
            .await;

        let notify = Arc::new(tokio::sync::Notify::new());
        server.register_reconnect_notify("192.168.1.10", notify.clone());

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/readers/192.168.1.10/reconnect",
                server.local_addr()
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        // Notify should have been fired — notified() should complete immediately
        tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified())
            .await
            .expect("notify should have been fired");
    }

    #[tokio::test]
    async fn reconnect_endpoint_returns_404_for_unknown_reader() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/readers/10.0.0.99/reconnect",
                server.local_addr()
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    /// Test that `run_status_poll_merge_successes` preserves cached estimated_stored_reads
    /// and recording when the follow-up extended status poll fails.
    #[tokio::test]
    async fn set_read_mode_preserves_stored_reads_when_ext_status_poll_fails() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;
        server
            .update_reader_state(reader_ip, ReaderConnectionState::Connected)
            .await;
        // Pre-populate with cached values we expect to be preserved
        server
            .update_reader_info(
                reader_ip,
                crate::reader_control::ReaderInfo {
                    config: Some(crate::reader_control::Config3Info {
                        mode: control::ReadMode::Raw,
                        timeout: 5,
                    }),
                    estimated_stored_reads: Some(42),
                    recording: Some(true),
                    tto_enabled: Some(false),
                    clock: Some(crate::reader_control::ClockInfo {
                        reader_clock: "2026-03-06T18:55:44.000".to_owned(),
                        drift_ms: 100,
                    }),
                    ..Default::default()
                },
            )
            .await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let feeder = tokio::spawn(async move {
            // 1. SetConfig3 command -> ACK
            let _set_cmd = cmd_rx.recv().await.expect("set config3 command");
            assert!(
                control_sink
                    .feed(ack_for(control::INSTR_CONFIG3).as_bytes())
                    .await
            );

            // 2. GetExtendedStatus -> send garbage (causes failure)
            let _ext_cmd = cmd_rx.recv().await.expect("ext status command");
            assert!(control_sink.feed(b"not-an-ext-status").await);

            // 3. GetConfig3 -> valid response
            let _cfg_cmd = cmd_rx.recv().await.expect("config3 command");
            assert!(
                control_sink
                    .feed(config3_response(control::ReadMode::Event, 7).as_bytes())
                    .await
            );

            // 4. GetTagMessageFormat -> valid response
            let _tag_cmd = cmd_rx.recv().await.expect("tag format command");
            let tag_format = TagMessageFormat {
                field_mask: 0xff,
                id_byte_mask: 0xfc,
                ascii_header_1: 0x61,
                ascii_header_2: 0x61,
                binary_header_1: 0xaa,
                binary_header_2: 0x00,
                trailer_1: 0x0d,
                trailer_2: 0x0a,
                separator: None,
            };
            assert!(
                control_sink
                    .feed(tag_message_format_response(&tag_format).as_bytes())
                    .await
            );

            // 5. GetDateTime -> valid response
            let _dt_cmd = cmd_rx.recv().await.expect("date/time command");
            assert!(control_sink.feed(b"ab000902260306051855443727cf").await);
        });

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "http://{}/api/v1/readers/{}/read-mode",
                server.local_addr(),
                reader_ip
            ))
            .header("content-type", "application/json")
            .body(r#"{"mode":"event","timeout":7}"#)
            .send()
            .await
            .expect("PUT read-mode");
        assert_eq!(resp.status(), StatusCode::OK);

        feeder.await.expect("response feeder task");

        // Verify: estimated_stored_reads and recording are preserved (not null)
        // because run_status_poll_merge_successes keeps cached values on failure
        let status = client
            .get(format!("http://{}/api/v1/status", server.local_addr()))
            .send()
            .await
            .expect("GET /api/v1/status");
        let body: serde_json::Value = status.json().await.expect("status json");
        let info = &body["readers"][0]["reader_info"];
        assert_eq!(info["config"]["mode"], "event", "config should be updated");
        assert_eq!(
            info["estimated_stored_reads"], 42,
            "estimated_stored_reads should be preserved from cache"
        );
        assert_eq!(
            info["recording"], true,
            "recording should be preserved from cache"
        );
    }

    /// Test that set_recording_handler returns the new recording state.
    #[tokio::test]
    async fn set_recording_on_returns_recording_true() {
        let reader_ip = "192.168.1.10";
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.init_readers(&[(reader_ip.to_owned(), 10010)]).await;

        let (cmd_tx, mut cmd_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
        let (control_client, control_sink) = crate::reader_control::ControlClient::new(cmd_tx);
        server
            .control_clients()
            .write()
            .expect("control client lock")
            .insert(reader_ip.to_owned(), Arc::new(control_client));

        let feeder = tokio::spawn(async move {
            // 1. SetRecordingState -> extended status response (recording bit set)
            let _cmd = cmd_rx.recv().await.expect("set recording command");
            // Extended status with recording_state byte = 0x01 (recording)
            assert!(
                control_sink
                    .feed(b"ab000d4b010b012f0000000059058f0c005a")
                    .await
            );

            // run_status_poll follow-up: GetExtendedStatus
            let _cmd = cmd_rx.recv().await.expect("ext status poll");
            assert!(
                control_sink
                    .feed(b"ab000d4b010b012f0000000059058f0c005a")
                    .await
            );

            // GetConfig3
            let _cmd = cmd_rx.recv().await.expect("config3 poll");
            assert!(
                control_sink
                    .feed(config3_response(control::ReadMode::Raw, 5).as_bytes())
                    .await
            );

            // GetTagMessageFormat
            let _cmd = cmd_rx.recv().await.expect("tag format poll");
            let tag_format = TagMessageFormat {
                field_mask: 0x7f,
                id_byte_mask: 0xfc,
                ascii_header_1: 0x61,
                ascii_header_2: 0x61,
                binary_header_1: 0xaa,
                binary_header_2: 0x00,
                trailer_1: 0x0d,
                trailer_2: 0x0a,
                separator: None,
            };
            assert!(
                control_sink
                    .feed(tag_message_format_response(&tag_format).as_bytes())
                    .await
            );

            // GetDateTime
            let _cmd = cmd_rx.recv().await.expect("date/time poll");
            assert!(control_sink.feed(b"ab000902260306051855443727cf").await);
        });

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "http://{}/api/v1/readers/{}/recording",
                server.local_addr(),
                reader_ip
            ))
            .header("content-type", "application/json")
            .body(r#"{"enabled":true}"#)
            .send()
            .await
            .expect("PUT recording");
        assert_eq!(resp.status(), StatusCode::OK);

        let body: serde_json::Value = resp.json().await.expect("json response");
        assert!(body["recording"].is_boolean());

        feeder.await.expect("response feeder task");
    }

    /// Test that set_recording returns 503 when reader is not connected.
    #[tokio::test]
    async fn set_recording_returns_503_when_reader_disconnected() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .put(format!(
                "http://{}/api/v1/readers/10.0.0.99/recording",
                server.local_addr()
            ))
            .header("content-type", "application/json")
            .body(r#"{"enabled":true}"#)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 503);
    }

    /// Test that clear_records returns 503 when reader is not connected.
    #[tokio::test]
    async fn clear_records_returns_503_when_reader_disconnected() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/readers/10.0.0.99/clear-records",
                server.local_addr()
            ))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 503);
    }

    #[tokio::test]
    async fn config_ups_section_accepts_valid_values() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        let (_token_dir, token_path) = temp_token_file();
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
server_url = "https://timing.example.com"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/config/ups",
                server.local_addr()
            ))
            .header("content-type", "application/json")
            .body(r#"{"enabled":true,"daemon_addr":"127.0.0.1:8423","poll_interval_secs":5,"upstream_heartbeat_secs":30}"#)
            .send()
            .await
            .expect("post config ups");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn config_ups_section_rejects_invalid_poll_interval() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        let (_token_dir, token_path) = temp_token_file();
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
server_url = "https://timing.example.com"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{}/api/v1/config/ups", server.local_addr()))
            .header("content-type", "application/json")
            .body(r#"{"poll_interval_secs":0}"#)
            .send()
            .await
            .expect("post config ups");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn config_ups_section_rejects_invalid_daemon_addr() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        let (_token_dir, token_path) = temp_token_file();
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
server_url = "https://timing.example.com"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{}/api/v1/config/ups", server.local_addr()))
            .header("content-type", "application/json")
            .body(r#"{"daemon_addr":"not-valid"}"#)
            .send()
            .await
            .expect("post config ups");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn config_ups_section_normalizes_blank_daemon_addr_to_default() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        let token_file = NamedTempFile::new().expect("create temp token");
        // Use forward slashes so the path is valid in TOML basic strings on all
        // platforms (backslashes are escape characters in TOML).
        let token_path = token_file.path().display().to_string().replace('\\', "/");
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
server_url = "https://timing.example.com"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{}/api/v1/config/ups", server.local_addr()))
            .header("content-type", "application/json")
            .body(r#"{"enabled":true,"daemon_addr":""}"#)
            .send()
            .await
            .expect("post config ups");
        assert_eq!(resp.status(), StatusCode::OK);

        let saved = std::fs::read_to_string(config_file.path()).expect("read saved config");
        let loaded =
            crate::config::load_config_from_str(&saved, config_file.path()).expect("load config");
        assert_eq!(loaded.ups.daemon_addr, "127.0.0.1:8423");
    }
}
