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
//!   (general, server, auth, journal, uplink, status_http, control, update, readers)
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
//! Uplink connectivity does NOT affect readiness.
//!
//! # Security
//! No authentication in v1.

use crate::storage::journal::Journal;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use rt_updater::UpdateStatus;
use rt_updater::workflow::{RealChecker, WorkflowState, run_check, run_download};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::convert::Infallible;
use std::future::Future;
use std::io::Write as _;
use std::net::{SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};

// ---------------------------------------------------------------------------
// Public config
// ---------------------------------------------------------------------------

/// Configuration for the status HTTP server.
#[derive(Debug, Clone)]
pub struct StatusConfig {
    /// Bind address, e.g. `"0.0.0.0:8080"`.
    pub bind: String,
    /// Forwarder software version (shown in status page).
    pub forwarder_version: String,
}

// ---------------------------------------------------------------------------
// Subsystem readiness
// ---------------------------------------------------------------------------

/// Connection state of a reader TCP socket.
#[derive(Debug, Clone, PartialEq)]
pub enum ReaderConnectionState {
    Connecting,
    Connected,
    Disconnected,
}

/// Per-reader status tracked in memory.
#[derive(Debug, Clone)]
pub struct ReaderStatus {
    pub state: ReaderConnectionState,
    pub last_seen: Option<Instant>,
    pub reads_since_restart: u64,
    pub reads_total: i64,
    /// The local port the forwarder listens on to re-expose reads from this reader.
    pub local_port: u16,
    /// The name of the current epoch (set via the server), if any.
    pub current_epoch_name: Option<String>,
    /// Control protocol info (firmware, clock, etc.) — populated on connect.
    pub reader_info: Option<crate::reader_control::ReaderInfo>,
}

/// Tracks local subsystem readiness for the `/readyz` endpoint.
///
/// Ready = config loaded + journal open + worker tasks started.
/// Uplink connectivity is explicitly excluded from readiness.
#[derive(Debug, Clone)]
pub struct SubsystemStatus {
    ready: bool,
    reason: Option<String>,
    /// Uplink state is tracked for the status page but does NOT affect readiness.
    uplink_connected: bool,
    forwarder_id: String,
    local_ip: Option<String>,
    readers: HashMap<String, ReaderStatus>,
    update_status: UpdateStatus,
    staged_update_path: Option<std::path::PathBuf>,
    pub update_mode: rt_updater::UpdateMode,
    /// Set to `true` when config is saved and the forwarder needs a restart to apply changes.
    restart_needed: bool,
}

impl SubsystemStatus {
    /// Create a fully-ready subsystem status.
    pub fn ready() -> Self {
        SubsystemStatus {
            ready: true,
            reason: None,
            uplink_connected: false,
            forwarder_id: String::new(),
            local_ip: None,
            readers: HashMap::new(),
            update_status: UpdateStatus::UpToDate,
            staged_update_path: None,
            update_mode: rt_updater::UpdateMode::default(),
            restart_needed: false,
        }
    }

    /// Create a not-ready subsystem status with a reason.
    pub fn not_ready(reason: String) -> Self {
        SubsystemStatus {
            ready: false,
            reason: Some(reason),
            uplink_connected: false,
            forwarder_id: String::new(),
            local_ip: None,
            readers: HashMap::new(),
            update_status: UpdateStatus::UpToDate,
            staged_update_path: None,
            update_mode: rt_updater::UpdateMode::default(),
            restart_needed: false,
        }
    }

    /// Set the uplink connection state (does NOT affect `/readyz` result).
    pub fn set_uplink_connected(&mut self, connected: bool) {
        self.uplink_connected = connected;
    }

    /// Return true if all local subsystems are ready.
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Return the uplink connection state.
    pub fn uplink_connected(&self) -> bool {
        self.uplink_connected
    }

    /// Return whether a restart is needed to apply saved config changes.
    pub fn restart_needed(&self) -> bool {
        self.restart_needed
    }

    /// Mark that a restart is needed to apply saved config changes.
    pub fn set_restart_needed(&mut self) {
        self.restart_needed = true;
    }
}

// ---------------------------------------------------------------------------
// StatusServer handle
// ---------------------------------------------------------------------------

/// Handle to the running status HTTP server.
#[derive(Clone)]
pub struct StatusServer {
    local_addr: SocketAddr,
    subsystem: Arc<Mutex<SubsystemStatus>>,
    ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
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

/// Holds the config file path and a write lock for read-modify-write operations.
pub struct ConfigState {
    pub path: std::path::PathBuf,
    pub(crate) write_lock: Mutex<()>,
}

impl ConfigState {
    pub fn new(path: std::path::PathBuf) -> Self {
        ConfigState {
            path,
            write_lock: Mutex::new(()),
        }
    }
}

struct AppState<J: JournalAccess + Send + 'static> {
    subsystem: Arc<Mutex<SubsystemStatus>>,
    journal: Arc<Mutex<J>>,
    version: Arc<String>,
    config_state: Option<Arc<ConfigState>>,
    restart_signal: Option<Arc<Notify>>,
    ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
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

impl<J: JournalAccess + Send + 'static> Clone for AppState<J> {
    fn clone(&self) -> Self {
        Self {
            subsystem: self.subsystem.clone(),
            journal: self.journal.clone(),
            version: self.version.clone(),
            config_state: self.config_state.clone(),
            restart_signal: self.restart_signal.clone(),
            ui_tx: self.ui_tx.clone(),
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

    /// Return a clone of the internal subsystem status Arc.
    pub fn subsystem_arc(&self) -> Arc<Mutex<SubsystemStatus>> {
        self.subsystem.clone()
    }

    /// Return a clone of the UI event broadcast sender.
    pub fn ui_sender(&self) -> tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent> {
        self.ui_tx.clone()
    }

    /// Return a clone of the shared UI logger Arc.
    pub fn logger(&self) -> Arc<rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>> {
        self.logger.clone()
    }

    /// Mark all local subsystems as ready.
    pub async fn set_ready(&self) {
        let mut ss = self.subsystem.lock().await;
        ss.ready = true;
        ss.reason = None;
        let _ = self
            .ui_tx
            .send(crate::ui_events::ForwarderUiEvent::StatusChanged {
                ready: ss.is_ready(),
                uplink_connected: ss.uplink_connected(),
                restart_needed: ss.restart_needed(),
            });
    }

    /// Mark that a restart is needed to apply saved config changes.
    pub async fn set_restart_needed(&self) {
        mark_restart_needed_and_emit(&self.subsystem, &self.ui_tx).await;
    }

    /// Return whether a restart is needed to apply saved config changes.
    pub async fn restart_needed(&self) -> bool {
        self.subsystem.lock().await.restart_needed()
    }

    /// Update the uplink connection state (does not affect readiness).
    pub async fn set_uplink_connected(&self, connected: bool) {
        let mut ss = self.subsystem.lock().await;
        ss.set_uplink_connected(connected);
        let _ = self
            .ui_tx
            .send(crate::ui_events::ForwarderUiEvent::StatusChanged {
                ready: ss.is_ready(),
                uplink_connected: connected,
                restart_needed: ss.restart_needed(),
            });
    }

    /// Set the forwarder ID (call once at startup).
    pub async fn set_forwarder_id(&self, id: &str) {
        self.subsystem.lock().await.forwarder_id = id.to_owned();
    }

    /// Set the detected local IP (call once at startup).
    pub async fn set_local_ip(&self, ip: Option<String>) {
        self.subsystem.lock().await.local_ip = ip;
    }

    /// Set the update mode (controls check-only vs check-and-download behavior).
    pub async fn set_update_mode(&self, mode: rt_updater::UpdateMode) {
        self.subsystem.lock().await.update_mode = mode;
    }

    /// Update the current rt-updater status (shown on `/update/status`).
    pub async fn set_update_status(&self, status: UpdateStatus) {
        self.subsystem.lock().await.update_status = status.clone();
        let _ = self
            .ui_tx
            .send(crate::ui_events::ForwarderUiEvent::UpdateStatusChanged { status });
    }

    /// Record the filesystem path of a downloaded update artifact ready to apply.
    pub async fn set_staged_update_path(&self, path: std::path::PathBuf) {
        self.subsystem.lock().await.staged_update_path = Some(path);
    }

    pub fn control_clients(
        &self,
    ) -> &Arc<std::sync::RwLock<HashMap<String, Arc<crate::reader_control::ControlClient>>>> {
        &self.control_clients
    }

    #[allow(clippy::type_complexity)]
    pub fn download_trackers(
        &self,
    ) -> &Arc<
        std::sync::RwLock<
            HashMap<String, Arc<tokio::sync::Mutex<crate::reader_control::DownloadTracker>>>,
        >,
    > {
        &self.download_trackers
    }

    pub fn register_download_tracker(
        &self,
        reader_ip: &str,
        tracker: Arc<tokio::sync::Mutex<crate::reader_control::DownloadTracker>>,
    ) {
        self.download_trackers
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(reader_ip.to_owned(), tracker);
    }

    pub fn deregister_download_tracker(&self, reader_ip: &str) {
        self.download_trackers
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(reader_ip);
    }

    pub fn register_reconnect_notify(&self, reader_ip: &str, notify: Arc<Notify>) {
        self.reconnect_notifies
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(reader_ip.to_owned(), notify);
    }

    pub fn deregister_reconnect_notify(&self, reader_ip: &str) {
        self.reconnect_notifies
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(reader_ip);
    }

    pub fn reconnect_notifies(&self) -> &Arc<std::sync::RwLock<HashMap<String, Arc<Notify>>>> {
        &self.reconnect_notifies
    }

    pub async fn update_reader_info(
        &self,
        reader_ip: &str,
        info: crate::reader_control::ReaderInfo,
    ) {
        let mut ss = self.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(reader_ip) {
            r.reader_info = Some(info.clone());
        }
        let _ = self
            .ui_tx
            .send(crate::ui_events::ForwarderUiEvent::ReaderInfoUpdated {
                ip: reader_ip.to_owned(),
                info,
            });
    }

    pub fn register_control_client(
        &self,
        reader_ip: &str,
        client: Arc<crate::reader_control::ControlClient>,
    ) {
        self.control_clients
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(reader_ip.to_owned(), client);
    }

    pub fn deregister_control_client(&self, reader_ip: &str) {
        self.control_clients
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(reader_ip);
    }

    /// Pre-populate all configured reader IPs as Disconnected.
    ///
    /// Each entry is `(reader_addr, local_port)` where `reader_addr` is `"ip:port"`
    /// and `local_port` is the port the forwarder listens on to re-expose reads.
    pub async fn init_readers(&self, readers: &[(String, u16)]) {
        let mut ss = self.subsystem.lock().await;
        for (addr, local_port) in readers {
            ss.readers.entry(addr.clone()).or_insert(ReaderStatus {
                state: ReaderConnectionState::Disconnected,
                last_seen: None,
                reads_since_restart: 0,
                reads_total: 0,
                local_port: *local_port,
                current_epoch_name: None,
                reader_info: None,
            });
        }
    }

    /// Seed a reader's total historical count from durable journal state.
    pub async fn set_reader_total(&self, reader_ip: &str, total: i64) {
        let mut ss = self.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(reader_ip) {
            r.reads_total = total;
        }
    }

    /// Set the current epoch name for a reader and broadcast a ReaderUpdated SSE event.
    pub async fn set_reader_epoch_name(&self, reader_ip: &str, name: Option<String>) {
        let mut ss = self.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(reader_ip) {
            r.current_epoch_name = name;
            let state_str = match &r.state {
                ReaderConnectionState::Connected => "connected",
                ReaderConnectionState::Connecting => "connecting",
                ReaderConnectionState::Disconnected => "disconnected",
            };
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                    ip: reader_ip.to_owned(),
                    state: state_str.to_owned(),
                    reads_session: r.reads_since_restart,
                    reads_total: r.reads_total,
                    last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                    local_port: r.local_port,
                    current_epoch_name: r.current_epoch_name.clone(),
                });
        }
    }

    /// Update a reader's connection state.
    pub async fn update_reader_state(&self, reader_ip: &str, state: ReaderConnectionState) {
        let mut ss = self.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(reader_ip) {
            r.state = state;
            let state_str = match &r.state {
                ReaderConnectionState::Connected => "connected",
                ReaderConnectionState::Connecting => "connecting",
                ReaderConnectionState::Disconnected => "disconnected",
            };
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                    ip: reader_ip.to_owned(),
                    state: state_str.to_owned(),
                    reads_session: r.reads_since_restart,
                    reads_total: r.reads_total,
                    last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                    local_port: r.local_port,
                    current_epoch_name: r.current_epoch_name.clone(),
                });
        }
    }

    /// Record a successful chip read for a reader.
    pub async fn record_read(&self, reader_ip: &str) {
        let mut ss = self.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(reader_ip) {
            r.reads_since_restart += 1;
            r.reads_total += 1;
            r.last_seen = Some(Instant::now());
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                    ip: reader_ip.to_owned(),
                    state: match &r.state {
                        ReaderConnectionState::Connected => "connected",
                        ReaderConnectionState::Connecting => "connecting",
                        ReaderConnectionState::Disconnected => "disconnected",
                    }
                    .to_owned(),
                    reads_session: r.reads_since_restart,
                    reads_total: r.reads_total,
                    last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                    local_port: r.local_port,
                    current_epoch_name: r.current_epoch_name.clone(),
                });
        }
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

        let (ui_tx, _) = tokio::sync::broadcast::channel(256);
        let logger = Arc::new(rt_ui_log::UiLogger::with_buffer(
            ui_tx.clone(),
            |entry| crate::ui_events::ForwarderUiEvent::LogEntry { entry },
            500,
        ));
        let subsystem = Arc::new(Mutex::new(subsystem));
        let control_clients = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let download_trackers = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let reconnect_notifies = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let state = AppState {
            subsystem: subsystem.clone(),
            journal,
            version: Arc::new(cfg.forwarder_version),
            config_state: None,
            restart_signal: None,
            ui_tx: ui_tx.clone(),
            logger: logger.clone(),
            control_clients: control_clients.clone(),
            download_trackers: download_trackers.clone(),
            reconnect_notifies: reconnect_notifies.clone(),
        };

        let app = build_router(state);
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                tracing::error!(error = %err, "status HTTP server fatal error");
            }
        });

        Ok(StatusServer {
            local_addr,
            subsystem,
            ui_tx,
            logger,
            control_clients,
            download_trackers,
            reconnect_notifies,
        })
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

        let (ui_tx, _) = tokio::sync::broadcast::channel(256);
        let logger = Arc::new(rt_ui_log::UiLogger::with_buffer(
            ui_tx.clone(),
            |entry| crate::ui_events::ForwarderUiEvent::LogEntry { entry },
            500,
        ));
        let subsystem = Arc::new(Mutex::new(subsystem));
        let control_clients = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let download_trackers = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let reconnect_notifies = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let state = AppState {
            subsystem: subsystem.clone(),
            journal,
            version: Arc::new(cfg.forwarder_version),
            config_state: Some(config_state),
            restart_signal: Some(restart_signal),
            ui_tx: ui_tx.clone(),
            logger: logger.clone(),
            control_clients: control_clients.clone(),
            download_trackers: download_trackers.clone(),
            reconnect_notifies: reconnect_notifies.clone(),
        };

        let app = build_router(state);
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                tracing::error!(error = %err, "status HTTP server fatal error");
            }
        });

        Ok(StatusServer {
            local_addr,
            subsystem,
            ui_tx,
            logger,
            control_clients,
            download_trackers,
            reconnect_notifies,
        })
    }
}

// ---------------------------------------------------------------------------
// JournalAccess trait (for epoch reset, testable with real Journal or NoJournal)
// ---------------------------------------------------------------------------

/// Trait that abstracts journal access for the epoch-reset endpoint.
pub trait JournalAccess {
    /// Bump the epoch for `stream_key`.
    ///
    /// Returns `Ok(new_epoch)` on success, `Err(NotFound)` if stream unknown.
    fn reset_epoch(&mut self, stream_key: &str) -> Result<i64, EpochResetError>;

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
    fn reset_epoch(&mut self, stream_key: &str) -> Result<i64, EpochResetError> {
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
        Ok(new_epoch)
    }

    fn event_count(&self, stream_key: &str) -> Result<i64, String> {
        Journal::event_count(self, stream_key).map_err(|e| e.to_string())
    }
}

/// Sentinel "no journal" implementation: every reset returns NotFound.
struct NoJournal;

impl JournalAccess for NoJournal {
    fn reset_epoch(&mut self, _stream_key: &str) -> Result<i64, EpochResetError> {
        Err(EpochResetError::NotFound)
    }

    fn event_count(&self, _stream_key: &str) -> Result<i64, String> {
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_atomic(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    let original_permissions = std::fs::metadata(path).map(|m| m.permissions()).ok();

    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path has no parent: {}", path.display()),
        )
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("path has no file name: {}", path.display()),
        )
    })?;

    let file_name = file_name.to_string_lossy();
    let pid = std::process::id();

    for attempt in 0..=16 {
        let tmp_name = format!(".{}.tmp.{}.{}", file_name, pid, attempt);
        let tmp_path = parent.join(tmp_name);
        match std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)
        {
            Ok(mut temp_file) => {
                let result = (|| -> std::io::Result<()> {
                    temp_file.write_all(content.as_bytes())?;
                    temp_file.sync_all()?;

                    if let Some(perms) = &original_permissions {
                        std::fs::set_permissions(&tmp_path, perms.clone())?;
                    }

                    std::fs::rename(&tmp_path, path)?;
                    if let Ok(parent_dir) = std::fs::File::open(parent) {
                        let _ = parent_dir.sync_all();
                    }
                    Ok(())
                })();
                if result.is_err() {
                    let _ = std::fs::remove_file(&tmp_path);
                }
                return result;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }

    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        format!("failed to allocate temp path for {}", path.display()),
    ))
}

/// Read the TOML config file, apply a mutation, and write it back.
///
/// Returns Ok(()) on success or Err((status_code, json_error_body)) on failure.
async fn update_config_file(
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
    mutate: impl FnOnce(&mut crate::config::RawConfig) -> Result<(), String>,
) -> Result<(), (u16, String)> {
    let _lock = config_state.write_lock.lock().await;

    let toml_str = std::fs::read_to_string(&config_state.path).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("File read error: {}", e)})
                .to_string(),
        )
    })?;

    let mut raw: crate::config::RawConfig = toml::from_str(&toml_str).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("TOML parse error: {}", e)})
                .to_string(),
        )
    })?;

    mutate(&mut raw).map_err(|e| {
        (
            400u16,
            serde_json::json!({"ok": false, "error": e}).to_string(),
        )
    })?;

    let new_toml = toml::to_string_pretty(&raw).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("TOML serialize error: {}", e)})
                .to_string(),
        )
    })?;

    write_atomic(&config_state.path, &new_toml).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("File write error: {}", e)})
                .to_string(),
        )
    })?;

    mark_restart_needed_and_emit(subsystem, ui_tx).await;
    Ok(())
}

async fn mark_restart_needed_and_emit(
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
) {
    let mut ss = subsystem.lock().await;
    ss.set_restart_needed();
    let _ = ui_tx.send(crate::ui_events::ForwarderUiEvent::StatusChanged {
        ready: ss.is_ready(),
        uplink_connected: ss.uplink_connected(),
        restart_needed: true,
    });
}

/// Apply a config section update by name.
///
/// Dispatches to the right mutation logic based on `section`, validates the
/// payload, and calls `update_config_file` to persist the change.
///
/// Recognised sections: `"general"`, `"server"`, `"auth"`, `"journal"`,
/// `"uplink"`, `"status_http"`, `"control"`, `"update"`, `"readers"`.
pub async fn apply_section_update(
    section: &str,
    payload: &serde_json::Value,
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
    logger: Option<&rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
) -> Result<(), (u16, String)> {
    require_object_payload(payload)?;

    match section {
        "general" => {
            let display_name = optional_string_field(payload, "display_name")?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.display_name = display_name;
                Ok(())
            })
            .await
        }
        "server" => {
            let base_url_opt = optional_string_field(payload, "base_url")?;
            let base_url =
                require_non_empty_trimmed("base_url", base_url_opt).map_err(bad_request_error)?;
            validate_base_url(&base_url).map_err(bad_request_error)?;
            let forwarders_ws_path = optional_string_field(payload, "forwarders_ws_path")?
                .and_then(|s| {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_owned())
                    }
                });
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.server = Some(crate::config::RawServerConfig {
                    base_url: Some(base_url),
                    forwarders_ws_path,
                });
                Ok(())
            })
            .await
        }
        "auth" => {
            let token_file_opt = optional_string_field(payload, "token_file")?;
            let token_file = require_non_empty_trimmed("token_file", token_file_opt)
                .map_err(bad_request_error)?;
            validate_token_file(&token_file).map_err(bad_request_error)?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.auth = Some(crate::config::RawAuthConfig {
                    token_file: Some(token_file),
                });
                Ok(())
            })
            .await
        }
        "journal" => {
            let sqlite_path = optional_string_field(payload, "sqlite_path")?;
            let prune_watermark_pct = optional_u8_field(payload, "prune_watermark_pct")?;
            if let Some(pct) = prune_watermark_pct
                && pct > 100
            {
                return Err(bad_request_error(
                    "prune_watermark_pct must be between 0 and 100",
                ));
            }
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.journal = Some(crate::config::RawJournalConfig {
                    sqlite_path,
                    prune_watermark_pct,
                });
                Ok(())
            })
            .await
        }
        "uplink" => {
            let batch_mode = optional_string_field(payload, "batch_mode")?;
            if let Some(ref mode) = batch_mode
                && mode != "immediate"
                && mode != "batched"
            {
                return Err(bad_request_error(
                    "batch_mode must be \"immediate\" or \"batched\"",
                ));
            }
            let batch_flush_ms = optional_u64_field(payload, "batch_flush_ms")?;
            let batch_max_events = optional_u32_field(payload, "batch_max_events")?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.uplink = Some(crate::config::RawUplinkConfig {
                    batch_mode,
                    batch_flush_ms,
                    batch_max_events,
                });
                Ok(())
            })
            .await
        }
        "status_http" => {
            let bind = optional_string_field(payload, "bind")?.and_then(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            });
            if let Some(ref bind_addr) = bind {
                validate_status_bind(bind_addr).map_err(bad_request_error)?;
            }
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.status_http = Some(crate::config::RawStatusHttpConfig { bind });
                Ok(())
            })
            .await
        }
        "control" => {
            let allow_power_actions = optional_bool_field(payload, "allow_power_actions")?;
            let action = optional_string_field(payload, "action")?;
            if let Some(action) = action {
                return apply_control_action_from_config(&action, config_state, logger).await;
            }
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.control = Some(crate::config::RawControlConfig {
                    allow_power_actions,
                });
                Ok(())
            })
            .await
        }
        "update" => {
            let mode_str = optional_string_field(payload, "mode")?;
            let parsed_mode = match mode_str.as_ref() {
                Some(m) => serde_json::from_value::<rt_updater::UpdateMode>(
                    serde_json::Value::String(m.clone()),
                )
                .map_err(|_| {
                    (
                        400u16,
                        serde_json::json!({"ok": false, "error": format!(
                            "mode must be 'disabled', 'check-only', or 'check-and-download', got '{}'", m
                        )})
                        .to_string(),
                    )
                })?,
                None => rt_updater::UpdateMode::default(),
            };
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.update = Some(crate::config::RawUpdateConfig { mode: mode_str });
                Ok(())
            })
            .await?;
            subsystem.lock().await.update_mode = parsed_mode;
            Ok(())
        }
        "readers" => {
            let readers_val = payload.get("readers").ok_or_else(|| {
                (
                    400u16,
                    serde_json::json!({"ok": false, "error": "readers field is required"})
                        .to_string(),
                )
            })?;
            let readers_arr = readers_val.as_array().ok_or_else(|| {
                (
                    400u16,
                    serde_json::json!({"ok": false, "error": "readers must be an array"})
                        .to_string(),
                )
            })?;

            if readers_arr.is_empty() {
                return Err((
                    400u16,
                    "{\"ok\":false,\"error\":\"at least one reader is required\"}".to_owned(),
                ));
            }

            let mut raw_readers = Vec::with_capacity(readers_arr.len());
            for (i, entry) in readers_arr.iter().enumerate() {
                let target = optional_string_field(entry, "target")?;

                let target_str = match &target {
                    Some(t) => t,
                    None => {
                        return Err((
                            400u16,
                            serde_json::json!({"ok": false, "error": format!("readers[{}].target is required", i)}).to_string(),
                        ));
                    }
                };

                if let Err(e) = crate::discovery::expand_target(target_str) {
                    return Err((
                        400u16,
                        serde_json::json!({"ok": false, "error": format!("readers[{}].target invalid: {}", i, e)}).to_string(),
                    ));
                }

                let enabled = optional_bool_field(entry, "enabled")?;
                let local_fallback_port = optional_u16_field(entry, "local_fallback_port")?;

                raw_readers.push(crate::config::RawReaderConfig {
                    target,
                    enabled,
                    local_fallback_port,
                });
            }

            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.readers = Some(raw_readers);
                Ok(())
            })
            .await
        }
        _ => Err((
            400u16,
            serde_json::json!({"ok": false, "error": format!("unknown section: {}", section)})
                .to_string(),
        )),
    }
}

/// Read the config TOML file as JSON.
///
/// Returns `(config_json, restart_needed)` on success.
pub async fn read_config_json(
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
) -> Result<(serde_json::Value, bool), (u16, String)> {
    let _lock = config_state.write_lock.lock().await;

    let toml_str = std::fs::read_to_string(&config_state.path).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("File read error: {}", e)})
                .to_string(),
        )
    })?;

    let raw: crate::config::RawConfig = toml::from_str(&toml_str).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("TOML parse error: {}", e)})
                .to_string(),
        )
    })?;

    let json = serde_json::to_value(&raw).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("JSON serialize error: {}", e)})
                .to_string(),
        )
    })?;

    let restart_needed = subsystem.lock().await.restart_needed();
    Ok((json, restart_needed))
}

fn text_response(status: StatusCode, body: impl Into<String>) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain")], body.into()).into_response()
}

fn json_response(status: StatusCode, body: String) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body).into_response()
}

fn parse_json_body<T: DeserializeOwned>(body: &Bytes) -> Result<T, String> {
    serde_json::from_slice::<T>(body).map_err(|e| format!("Invalid JSON: {}", e))
}

fn bad_request_error(message: impl Into<String>) -> (u16, String) {
    (
        400u16,
        serde_json::json!({"ok": false, "error": message.into()}).to_string(),
    )
}

fn require_object_payload(payload: &serde_json::Value) -> Result<(), (u16, String)> {
    if payload.is_object() {
        Ok(())
    } else {
        Err(bad_request_error("payload must be a JSON object"))
    }
}

fn optional_string_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<String>, (u16, String)> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(bad_request_error(format!(
            "{} must be a string or null",
            field
        ))),
    }
}

fn optional_bool_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<bool>, (u16, String)> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad_request_error(format!(
            "{} must be a boolean or null",
            field
        ))),
    }
}

fn optional_u64_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u64>, (u16, String)> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => {
            let raw = value.as_u64().ok_or_else(|| {
                bad_request_error(format!("{} must be a non-negative integer or null", field))
            })?;
            Ok(Some(raw))
        }
    }
}

fn optional_u32_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u32>, (u16, String)> {
    let raw = optional_u64_field(payload, field)?;
    raw.map(|value| {
        u32::try_from(value)
            .map_err(|_| bad_request_error(format!("{} must be <= {}", field, u32::MAX)))
    })
    .transpose()
}

fn optional_u16_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u16>, (u16, String)> {
    let raw = optional_u64_field(payload, field)?;
    raw.map(|value| {
        u16::try_from(value)
            .map_err(|_| bad_request_error(format!("{} must be <= {}", field, u16::MAX)))
    })
    .transpose()
}

fn optional_u8_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u8>, (u16, String)> {
    let raw = optional_u64_field(payload, field)?;
    raw.map(|value| {
        u8::try_from(value)
            .map_err(|_| bad_request_error(format!("{} must be <= {}", field, u8::MAX)))
    })
    .transpose()
}

fn require_non_empty_trimmed(field: &str, value: Option<String>) -> Result<String, String> {
    let raw = value.ok_or_else(|| format!("{} is required", field))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{} must not be empty", field));
    }
    Ok(trimmed.to_owned())
}

fn validate_base_url(base_url: &str) -> Result<(), String> {
    let uri: Uri = base_url
        .parse()
        .map_err(|_| "base_url must be a valid absolute URL".to_owned())?;
    let scheme = uri
        .scheme_str()
        .ok_or_else(|| "base_url must include scheme".to_owned())?;
    if scheme != "http" && scheme != "https" {
        return Err("base_url scheme must be http or https".to_owned());
    }
    if uri.authority().is_none() {
        return Err("base_url must include host".to_owned());
    }
    Ok(())
}

fn validate_token_file(token_file: &str) -> Result<(), String> {
    if token_file.contains('\n') || token_file.contains('\r') {
        return Err("token_file must be a single-line path".to_owned());
    }
    Ok(())
}

fn validate_status_bind(bind: &str) -> Result<(), String> {
    bind.parse::<SocketAddrV4>()
        .map(|_| ())
        .map_err(|_| "bind must be a valid IPv4 address with port (e.g. 0.0.0.0:8080)".to_owned())
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

async fn read_allow_power_actions(config_state: &ConfigState) -> Result<bool, (u16, String)> {
    let _lock = config_state.write_lock.lock().await;
    let toml_str = std::fs::read_to_string(&config_state.path).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("File read error: {}", e)})
                .to_string(),
        )
    })?;
    let raw: crate::config::RawConfig = toml::from_str(&toml_str).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("TOML parse error: {}", e)})
                .to_string(),
        )
    })?;
    Ok(raw
        .control
        .and_then(|c| c.allow_power_actions)
        .unwrap_or(false))
}

#[cfg(unix)]
fn map_power_action_command_result(
    systemctl_action: &'static str,
    result: std::io::Result<std::process::Output>,
    logger: Option<&rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
) -> Result<(), (u16, String)> {
    match result {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let detail = power_action_command_detail(&output);
            let status_code = if power_action_auth_failed(&detail) {
                403u16
            } else {
                500u16
            };
            tracing::error!(
                action = systemctl_action,
                exit_status = ?output.status.code(),
                detail = %detail,
                "control action command exited with failure"
            );
            if let Some(logger) = logger {
                logger.log_at(
                    rt_ui_log::UiLogLevel::Error,
                    format!(
                        "systemctl {} exited with failure (code {:?})",
                        systemctl_action,
                        output.status.code(),
                    ),
                );
            }
            Err((
                status_code,
                serde_json::json!({
                    "ok": false,
                    "error": format!(
                        "control action command exited with failure: systemctl {} ({})",
                        systemctl_action,
                        detail
                    )
                })
                .to_string(),
            ))
        }
        Err(e) => {
            tracing::error!(action = systemctl_action, error = %e, "control action command failed");
            if let Some(logger) = logger {
                logger.log_at(
                    rt_ui_log::UiLogLevel::Error,
                    format!("systemctl {} failed: {}", systemctl_action, e),
                );
            }
            Err((
                500u16,
                serde_json::json!({
                    "ok": false,
                    "error": format!("control action command failed: {}", e)
                })
                .to_string(),
            ))
        }
    }
}

#[cfg(unix)]
fn map_power_action_join_error(
    systemctl_action: &'static str,
    e: tokio::task::JoinError,
    logger: Option<&rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
) -> (u16, String) {
    tracing::error!(action = systemctl_action, error = %e, "control action task failed");
    if let Some(logger) = logger {
        logger.log_at(
            rt_ui_log::UiLogLevel::Error,
            format!("systemctl {} task failed: {}", systemctl_action, e),
        );
    }
    (
        500u16,
        serde_json::json!({
            "ok": false,
            "error": format!("control action task failed: {}", e)
        })
        .to_string(),
    )
}

#[cfg(unix)]
async fn run_device_power_action(
    systemctl_action: &'static str,
    logger: Option<&rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
) -> Result<(), (u16, String)> {
    match tokio::task::spawn_blocking(move || run_power_action_command(systemctl_action)).await {
        Ok(result) => map_power_action_command_result(systemctl_action, result, logger),
        Err(e) => Err(map_power_action_join_error(systemctl_action, e, logger)),
    }
}

#[cfg(unix)]
fn run_power_action_command(
    systemctl_action: &'static str,
) -> std::io::Result<std::process::Output> {
    std::process::Command::new("systemctl")
        .arg("--no-ask-password")
        .arg(systemctl_action)
        .output()
}

#[cfg(unix)]
fn power_action_command_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !stdout.is_empty() {
        return stdout;
    }
    "no command output".to_owned()
}

#[cfg(unix)]
fn power_action_auth_failed(detail: &str) -> bool {
    let lower = detail.to_ascii_lowercase();
    lower.contains("interactive authentication required")
        || lower.contains("authentication is required")
        || lower.contains("not authorized")
        || lower.contains("access denied")
        || lower.contains("permission denied")
        || lower.contains("a password is required")
}

#[cfg(not(unix))]
async fn run_device_power_action(
    _systemctl_action: &'static str,
    _logger: Option<&rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
) -> Result<(), (u16, String)> {
    Err((
        501u16,
        serde_json::json!({
            "ok": false,
            "error": "power actions not supported on non-unix platforms"
        })
        .to_string(),
    ))
}

async fn apply_control_action_from_config(
    action: &str,
    config_state: &ConfigState,
    logger: Option<&rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
) -> Result<(), (u16, String)> {
    apply_control_action_from_config_with(
        action,
        config_state,
        logger,
        |action, config_state, restart_signal, logger| {
            Box::pin(async move {
                apply_control_action(action, config_state, restart_signal, logger).await
            })
        },
    )
    .await
}

async fn apply_control_action_from_config_with<F>(
    action: &str,
    config_state: &ConfigState,
    logger: Option<&rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
    apply_fn: F,
) -> Result<(), (u16, String)>
where
    F: for<'a> FnOnce(
        &'a str,
        Option<&'a ConfigState>,
        Option<&'a Arc<Notify>>,
        Option<&'a rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
    ) -> Pin<Box<dyn Future<Output = Result<(), (u16, String)>> + Send + 'a>>,
{
    apply_fn(action, Some(config_state), None, logger).await
}

pub async fn apply_control_action(
    action: &str,
    config_state: Option<&ConfigState>,
    restart_signal: Option<&Arc<Notify>>,
    logger: Option<&rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
) -> Result<(), (u16, String)> {
    match action {
        "restart_service" => {
            let signal = restart_signal.ok_or_else(|| {
                (
                    404u16,
                    serde_json::json!({"ok": false, "error": "restart signal not available"})
                        .to_string(),
                )
            })?;
            if cfg!(unix) {
                signal.notify_one();
                Ok(())
            } else {
                Err((
                    501u16,
                    serde_json::json!({
                        "ok": false,
                        "error": "restart not supported on non-unix platforms"
                    })
                    .to_string(),
                ))
            }
        }
        "restart_device" | "shutdown_device" => {
            let cs = config_state.ok_or_else(|| {
                (
                    404u16,
                    serde_json::json!({"ok": false, "error": "Config editing not available"})
                        .to_string(),
                )
            })?;
            let allow_power_actions = read_allow_power_actions(cs).await?;
            if !allow_power_actions {
                return Err((
                    403u16,
                    serde_json::json!({"ok": false, "error": "power actions disabled"}).to_string(),
                ));
            }
            if !cfg!(unix) {
                return Err((
                    501u16,
                    serde_json::json!({
                        "ok": false,
                        "error": format!("{} not supported on non-unix platforms", action)
                    })
                    .to_string(),
                ));
            }
            let systemctl_action = if action == "restart_device" {
                "reboot"
            } else {
                "poweroff"
            };
            run_device_power_action(systemctl_action, logger).await?;
            Ok(())
        }
        _ => Err(bad_request_error(format!(
            "unknown control action: {}",
            action
        ))),
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
    uplink_connected: bool,
    restart_needed: bool,
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
    current_epoch_name: Option<String>,
    reader_info: Option<crate::reader_control::ReaderInfo>,
}

async fn status_json_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let ss = state.subsystem.lock().await;
    let mut readers: Vec<_> = ss
        .readers
        .iter()
        .map(|(ip, r)| {
            let state_str = match r.state {
                ReaderConnectionState::Connected => "connected",
                ReaderConnectionState::Connecting => "connecting",
                ReaderConnectionState::Disconnected => "disconnected",
            };
            ReaderStatusJson {
                ip: ip.clone(),
                state: state_str.to_owned(),
                reads_session: r.reads_since_restart,
                reads_total: r.reads_total,
                last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                local_port: r.local_port,
                current_epoch_name: r.current_epoch_name.clone(),
                reader_info: r.reader_info.clone(),
            }
        })
        .collect();
    readers.sort_by(|a, b| a.ip.cmp(&b.ip));

    let resp = StatusJsonResponse {
        forwarder_id: ss.forwarder_id.clone(),
        version: (*state.version).clone(),
        ready: ss.is_ready(),
        ready_reason: ss.reason.clone(),
        uplink_connected: ss.uplink_connected(),
        restart_needed: ss.restart_needed(),
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
    use std::time::Duration;
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
            Some(info) => json_response(
                StatusCode::OK,
                serde_json::to_string(info).unwrap_or_else(|_| "{}".to_owned()),
            ),
            None => json_response(StatusCode::OK, "{}".to_owned()),
        },
        None => text_response(StatusCode::NOT_FOUND, "unknown reader"),
    }
}

/// Estimate one-way network latency to a reader by measuring RTT of GET_DATE_TIME probes.
/// Returns (median one-way latency, successful probe count) from 3 probes.
async fn estimate_one_way_latency(
    client: &crate::reader_control::ControlClient,
) -> Result<(std::time::Duration, usize), String> {
    const PROBES: usize = 3;
    let mut rtts = Vec::with_capacity(PROBES);
    for i in 0..PROBES {
        let start = std::time::Instant::now();
        match client.get_date_time().await {
            Ok(_) => rtts.push(start.elapsed()),
            Err(e) => tracing::warn!(probe = i + 1, error = %e, "RTT probe failed"),
        }
    }
    if rtts.is_empty() {
        return Err(
            "all RTT probes failed; cannot estimate network latency for clock sync".to_string(),
        );
    }
    if rtts.len() < PROBES {
        tracing::warn!(
            successful = rtts.len(),
            total = PROBES,
            "clock sync latency estimate based on fewer probes than requested"
        );
    }
    rtts.sort();
    let median_rtt = rtts[rtts.len() / 2];
    Ok((median_rtt / 2, rtts.len()))
}

/// POST /api/v1/readers/{ip}/sync-clock
///
/// Minimizes clock drift by:
/// 1. Probing RTT to estimate one-way network latency
/// 2. Reading the reader's clock to learn its centisecond phase
/// 3. Choosing the SET_DATE_TIME second value that minimizes drift, accounting
///    for the fact that SET_DATE_TIME takes effect at the next centisecond
///    rollover (next second boundary), not immediately.
///
/// The reader's centisecond counter free-runs through SET_DATE_TIME. The new
/// second value is applied when the cs counter next wraps to 0. At that moment
/// the reader shows S.000. We pick S = round(wall_at_rollover) so that
/// |drift| ≤ 500ms worst case, ~250ms average.
async fn sync_clock_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    let client = {
        state
            .control_clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    let Some(client) = client else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected");
    };

    use chrono::Datelike;
    use chrono::Timelike;

    // Step 1: estimate one-way latency
    let (one_way, _probes) = match estimate_one_way_latency(&client).await {
        Ok(pair) => pair,
        Err(msg) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": msg}).to_string(),
            );
        }
    };

    // Step 2: read the reader's clock to learn its centisecond phase
    let dt = match client.get_date_time().await {
        Ok(dt) => dt,
        Err(e) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": format!("failed to read reader clock: {}", e)})
                    .to_string(),
            );
        }
    };
    let wall_now = chrono::Local::now();

    // The reader sampled its clock ~one_way ago. The SET_DATE_TIME we're about
    // to send will arrive ~one_way from now. So from sample to arrival ≈ 2*one_way.
    // Reader centisecond at arrival: (cs*10 + 2*one_way_ms) mod 1000
    let cs = dt.centisecond as f64;
    let reader_cs_at_arrival_ms = (cs * 10.0 + one_way.as_secs_f64() * 2000.0) % 1000.0;
    let reader_frac = reader_cs_at_arrival_ms / 1000.0; // 0.0 .. 1.0

    // Wall clock at the moment the command arrives
    let arrival_offset = chrono::Duration::from_std(one_way).unwrap_or(chrono::Duration::zero());
    let arrival_wall = wall_now + arrival_offset;

    // Step 3: choose the second value that minimizes drift.
    // SET_DATE_TIME takes effect at the next cs rollover (when cs wraps to 0),
    // NOT immediately. At that moment the reader shows S.000.
    // Time from arrival to rollover = (1.0 - reader_frac) seconds.
    let rollover_delay_ms = ((1.0 - reader_frac) * 1000.0) as i64;
    let wall_at_rollover = arrival_wall + chrono::Duration::milliseconds(rollover_delay_ms);
    let rollover_frac = wall_at_rollover.nanosecond() as f64 / 1_000_000_000.0;

    // Pick S = round(wall_at_rollover) so that |S.000 - wall_at_rollover| ≤ 500ms
    let target = if rollover_frac >= 0.5 {
        wall_at_rollover + chrono::Duration::seconds(1)
    } else {
        wall_at_rollover
    };

    let year = (target.year() % 100) as u8;
    let month = target.month() as u8;
    let day = target.day() as u8;
    let dow = target.weekday().num_days_from_sunday() as u8;
    let hour = target.hour() as u8;
    let minute = target.minute() as u8;
    let second = target.second() as u8;

    if let Err(e) = client
        .set_date_time(year, month, day, dow, hour, minute, second)
        .await
    {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}).to_string(),
        );
    }

    match client.get_date_time().await {
        Ok(dt) => {
            let reader_iso = dt.to_iso_string();
            let verify_now = chrono::Local::now();
            let drift_ms =
                chrono::NaiveDateTime::parse_from_str(&reader_iso, "%Y-%m-%dT%H:%M:%S%.3f")
                    .ok()
                    .map(|reader_naive| {
                        verify_now
                            .naive_local()
                            .signed_duration_since(reader_naive)
                            .num_milliseconds()
                    });

            // Update stored reader_info so SSE subscribers see the new clock
            {
                let mut ss = state.subsystem.lock().await;
                if let Some(r) = ss.readers.get_mut(&ip)
                    && let Some(ref mut info) = r.reader_info
                {
                    if let Some(d) = drift_ms {
                        info.clock = Some(crate::reader_control::ClockInfo {
                            reader_clock: reader_iso.clone(),
                            drift_ms: d,
                        });
                    } else {
                        info.clock = None;
                    }
                }
            }

            state.logger.log(format!(
                "reader {} clock synced to {} (one-way latency: {:.1}ms, cs phase: {:.0}, rollover frac: {:.0}ms)",
                ip,
                reader_iso,
                one_way.as_secs_f64() * 1000.0,
                cs,
                rollover_frac * 1000.0,
            ));
            json_response(
                StatusCode::OK,
                serde_json::json!({"reader_clock": reader_iso, "clock_drift_ms": drift_ms})
                    .to_string(),
            )
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": format!("set ok but verify failed: {}", e)}).to_string(),
        ),
    }
}

async fn update_cached_reader_info<J: JournalAccess + Send + 'static>(
    state: &AppState<J>,
    ip: &str,
    info: crate::reader_control::ReaderInfo,
) {
    {
        let mut ss = state.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(ip) {
            r.reader_info = Some(info.clone());
        }
    }
    let _ = state
        .ui_tx
        .send(crate::ui_events::ForwarderUiEvent::ReaderInfoUpdated {
            ip: ip.to_owned(),
            info,
        });
}

/// GET /api/v1/readers/{ip}/read-mode
async fn get_read_mode_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    let client = {
        state
            .control_clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    let Some(client) = client else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected");
    };
    match client.get_config3().await {
        Ok((mode, timeout)) => json_response(
            StatusCode::OK,
            serde_json::json!({"mode": mode.as_str(), "timeout": timeout}).to_string(),
        ),
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}).to_string(),
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
    let client = {
        state
            .control_clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    let Some(client) = client else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected");
    };
    let mode = match body.mode.as_str() {
        "raw" => ipico_core::control::ReadMode::Raw,
        "event" => ipico_core::control::ReadMode::Event,
        "fsls" => ipico_core::control::ReadMode::FirstLastSeen,
        _ => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({
                    "error": format!("unknown mode: {}", body.mode)
                })
                .to_string(),
            );
        }
    };
    match client.set_config3(mode, body.timeout).await {
        Ok(()) => {
            state
                .logger
                .log(format!("reader {} read mode set to {}", ip, mode));
            json_response(
                StatusCode::OK,
                serde_json::json!({"mode": mode.as_str()}).to_string(),
            )
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

/// GET /api/v1/readers/{ip}/tto
async fn get_tto_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    let client = {
        state
            .control_clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    let Some(client) = client else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected");
    };
    match client.get_tag_message_format().await {
        Ok(format) => json_response(
            StatusCode::OK,
            serde_json::json!({"enabled": format.tto_enabled()}).to_string(),
        ),
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

/// PUT /api/v1/readers/{ip}/tto
async fn set_tto_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
    axum::Json(body): axum::Json<SetTtoBody>,
) -> Response {
    let client = {
        state
            .control_clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    let Some(client) = client else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected");
    };

    let current = match client.get_tag_message_format().await {
        Ok(format) => format,
        Err(e) => {
            return json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": e.to_string()}).to_string(),
            );
        }
    };
    let updated = current.with_tto_enabled(body.enabled);
    if let Err(e) = client.set_tag_message_format(updated).await {
        return json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}).to_string(),
        );
    }

    match client.get_tag_message_format().await {
        Ok(format) => {
            let enabled = format.tto_enabled();
            let mut info = {
                let ss = state.subsystem.lock().await;
                ss.readers
                    .get(&ip)
                    .and_then(|r| r.reader_info.clone())
                    .unwrap_or_default()
            };
            info.tto_enabled = Some(enabled);
            update_cached_reader_info(&state, &ip, info).await;
            let label = if enabled { "enabled" } else { "disabled" };
            state
                .logger
                .log(format!("reader {} TTO reporting {}", ip, label));
            json_response(
                StatusCode::OK,
                serde_json::json!({"enabled": enabled}).to_string(),
            )
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": format!("set ok but verify failed: {}", e)}).to_string(),
        ),
    }
}

/// POST /api/v1/readers/{ip}/refresh
async fn refresh_handler_reader<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    let client = {
        state
            .control_clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    let Some(client) = client else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected");
    };
    let mut info = {
        let ss = state.subsystem.lock().await;
        ss.readers
            .get(&ip)
            .and_then(|r| r.reader_info.clone())
            .unwrap_or_default()
    };
    crate::reader_control::run_status_poll(&client, &mut info).await;
    update_cached_reader_info(&state, &ip, info.clone()).await;
    json_response(
        StatusCode::OK,
        serde_json::to_string(&info).unwrap_or_else(|_| "{}".to_owned()),
    )
}

/// POST /api/v1/readers/{ip}/clear-records
async fn clear_records_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    let client = {
        state
            .control_clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    let Some(client) = client else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected");
    };
    state
        .logger
        .log(format!("reader {} clearing onboard records...", ip));
    match client.clear_records().await {
        Ok(()) => {
            state.logger.log(format!("reader {} records cleared", ip));
            json_response(StatusCode::OK, "{\"ok\":true}".to_owned())
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

/// PUT /api/v1/readers/{ip}/recording
async fn set_recording_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
    axum::Json(body): axum::Json<SetRecordingBody>,
) -> Response {
    let client = {
        state
            .control_clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    let Some(client) = client else {
        return text_response(StatusCode::SERVICE_UNAVAILABLE, "reader not connected");
    };
    let label = if body.enabled { "on" } else { "off" };
    state
        .logger
        .log(format!("reader {} setting recording {}", ip, label));
    match client.set_recording(body.enabled).await {
        Ok(_ext) => {
            // Refresh full status so SSE subscribers see the new recording state
            let mut info = {
                let ss = state.subsystem.lock().await;
                ss.readers
                    .get(&ip)
                    .and_then(|r| r.reader_info.clone())
                    .unwrap_or_default()
            };
            crate::reader_control::run_status_poll(&client, &mut info).await;
            update_cached_reader_info(&state, &ip, info.clone()).await;
            let recording = info.recording.unwrap_or(false);
            json_response(
                StatusCode::OK,
                serde_json::json!({"recording": recording}).to_string(),
            )
        }
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"error": e.to_string()}).to_string(),
        ),
    }
}

/// POST /api/v1/readers/{ip}/reconnect
async fn reconnect_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(ip): Path<String>,
) -> Response {
    let notify = {
        state
            .reconnect_notifies
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    match notify {
        Some(n) => {
            n.notify_one();
            json_response(StatusCode::OK, r#"{"ok":true}"#.to_string())
        }
        None => json_response(
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
    let client = {
        state
            .control_clients
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&ip)
            .cloned()
    };
    let Some(client) = client else {
        return json_response(
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"reader not connected"}"#.to_string(),
        );
    };
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
            StatusCode::SERVICE_UNAVAILABLE,
            r#"{"error":"reader not connected"}"#.to_string(),
        );
    };

    // Check current state and prepare
    {
        let mut dt = tracker.lock().await;
        match dt.state() {
            crate::reader_control::DownloadState::Starting
            | crate::reader_control::DownloadState::Downloading => {
                return json_response(
                    StatusCode::CONFLICT,
                    r#"{"error":"download already in progress"}"#.to_string(),
                );
            }
            crate::reader_control::DownloadState::Complete
            | crate::reader_control::DownloadState::Error(_) => {
                dt.reset();
            }
            crate::reader_control::DownloadState::Idle => {}
        }
        dt.begin_startup();
    }

    // Get estimated_stored_reads from reader_info
    let estimated_reads = {
        let ss = state.subsystem.lock().await;
        ss.readers
            .get(&ip)
            .and_then(|r| r.reader_info.as_ref())
            .and_then(|ri| ri.estimated_stored_reads)
            .unwrap_or(0)
    };

    // Spawn background task to initiate the download
    let bg_tracker = tracker.clone();
    let bg_ip = ip.clone();
    tokio::spawn(async move {
        match client.start_download().await {
            Ok(ext) => {
                let mut dt = bg_tracker.lock().await;
                dt.start(ext.stored_data_extent);
            }
            Err(e) => {
                tracing::warn!(reader_ip = %bg_ip, error = %e, "download start failed");
                let mut dt = bg_tracker.lock().await;
                dt.fail(format!("{}", e));
            }
        }
    });

    json_response(
        StatusCode::ACCEPTED,
        serde_json::json!({"status": "started", "estimated_reads": estimated_reads}).to_string(),
    )
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
                .unwrap_or_else(|e| format!(r#"{{"state":"error","message":"serialize: {e}"}}"#));
            yield Ok::<_, Infallible>(SseEvent::default().data(json));
            return;
        }

        // Stream events from the broadcast channel
        loop {
            match rx.recv().await {
                Ok(evt) => {
                    let is_terminal = matches!(
                        evt,
                        crate::reader_control::DownloadEvent::Complete { .. }
                            | crate::reader_control::DownloadEvent::Error { .. }
                    );
                    let json = serde_json::to_string(&evt)
                        .unwrap_or_else(|e| format!(r#"{{"state":"error","message":"serialize: {e}"}}"#));
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
    Router::new()
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
        .route(
            "/api/v1/config/server",
            post(post_config_server_handler::<J>),
        )
        .route("/api/v1/config/auth", post(post_config_auth_handler::<J>))
        .route(
            "/api/v1/config/journal",
            post(post_config_journal_handler::<J>),
        )
        .route(
            "/api/v1/config/uplink",
            post(post_config_uplink_handler::<J>),
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
        .route(
            "/api/v1/config/readers",
            post(post_config_readers_handler::<J>),
        )
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
        Ok(new_epoch) => {
            state
                .logger
                .log(format!("epoch reset for {} via API", reader_ip));
            let body = serde_json::json!({"new_epoch": new_epoch}).to_string();
            json_response(StatusCode::OK, body)
        }
        Err(EpochResetError::NotFound) => text_response(StatusCode::NOT_FOUND, "stream not found"),
        Err(EpochResetError::Storage(e)) => text_response(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
}

#[derive(Debug, Deserialize)]
struct ServerStreamsResponse {
    streams: Vec<ServerStreamInfo>,
}

#[derive(Debug, Deserialize)]
struct ServerStreamInfo {
    stream_id: String,
    forwarder_id: String,
    reader_ip: String,
    stream_epoch: i64,
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

    let config_state = match &state.config_state {
        Some(config_state) => config_state.clone(),
        None => return config_not_available(),
    };

    let cfg = match crate::config::load_config_from_path(&config_state.path) {
        Ok(cfg) => cfg,
        Err(error) => {
            return text_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to load config: {error}"),
            );
        }
    };

    let forwarder_id = {
        let ss = state.subsystem.lock().await;
        ss.forwarder_id.clone()
    };
    if forwarder_id.is_empty() {
        return text_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "forwarder_id is not initialized",
        );
    }

    let base_url = cfg.server.base_url.trim_end_matches('/');
    let client = reqwest::Client::new();
    let streams_url = format!("{base_url}/api/v1/streams");
    let streams_resp = match client
        .get(&streams_url)
        .bearer_auth(&cfg.token)
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(error) => {
            return text_response(
                StatusCode::BAD_GATEWAY,
                format!("upstream stream lookup failed: {error}"),
            );
        }
    };

    let streams_status = streams_resp.status();
    let streams_body = match streams_resp.text().await {
        Ok(body) => body,
        Err(error) => {
            return text_response(
                StatusCode::BAD_GATEWAY,
                format!("upstream stream lookup body read failed: {error}"),
            );
        }
    };
    if !streams_status.is_success() {
        return text_response(
            StatusCode::BAD_GATEWAY,
            format!("upstream stream lookup returned {streams_status}: {streams_body}"),
        );
    }

    let streams = match serde_json::from_str::<ServerStreamsResponse>(&streams_body) {
        Ok(parsed) => parsed.streams,
        Err(error) => {
            return text_response(
                StatusCode::BAD_GATEWAY,
                format!("invalid upstream stream lookup response: {error}"),
            );
        }
    };
    let maybe_stream = streams
        .iter()
        .find(|stream| stream.forwarder_id == forwarder_id && stream.reader_ip == reader_ip);
    let stream = match maybe_stream {
        Some(stream) => stream,
        None => return text_response(StatusCode::NOT_FOUND, "stream not found"),
    };

    let epoch_name_url = format!(
        "{base_url}/api/v1/streams/{}/epochs/{}/name",
        stream.stream_id, stream.stream_epoch
    );
    let epoch_name_resp = match client
        .put(&epoch_name_url)
        .bearer_auth(&cfg.token)
        .json(&serde_json::json!({ "name": normalized_name }))
        .send()
        .await
    {
        Ok(resp) => resp,
        Err(error) => {
            return text_response(
                StatusCode::BAD_GATEWAY,
                format!("upstream epoch-name update failed: {error}"),
            );
        }
    };

    let response_status = status_from_u16_or_internal(epoch_name_resp.status().as_u16());
    let response_body = match epoch_name_resp.text().await {
        Ok(body) => body,
        Err(error) => {
            return text_response(
                StatusCode::BAD_GATEWAY,
                format!("upstream epoch-name response body read failed: {error}"),
            );
        }
    };
    if response_status.is_success() {
        state
            .logger
            .log(format!("set current epoch name for {} via API", reader_ip));

        // Store the epoch name locally and broadcast a ReaderUpdated SSE event.
        let mut ss = state.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(&reader_ip) {
            r.current_epoch_name = normalized_name;
            let state_str = match &r.state {
                ReaderConnectionState::Connected => "connected",
                ReaderConnectionState::Connecting => "connecting",
                ReaderConnectionState::Disconnected => "disconnected",
            };
            let _ = state
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                    ip: reader_ip.to_owned(),
                    state: state_str.to_owned(),
                    reads_session: r.reads_since_restart,
                    reads_total: r.reads_total,
                    last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                    local_port: r.local_port,
                    current_epoch_name: r.current_epoch_name.clone(),
                });
        }
        drop(ss);

        return json_response(response_status, response_body);
    }

    json_response(
        response_status,
        serde_json::json!({"error": response_body}).to_string(),
    )
}

fn status_from_u16_or_internal(status: u16) -> StatusCode {
    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

async fn update_status_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let update_status = {
        let ss = state.subsystem.lock().await;
        ss.update_status.clone()
    };
    let body = serde_json::to_string(&update_status)
        .unwrap_or_else(|_| r#"{"status":"failed","error":"serialization error"}"#.to_owned());
    json_response(StatusCode::OK, body)
}

async fn update_apply_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let ss = state.subsystem.lock().await;
    match &ss.staged_update_path {
        Some(path) => {
            let path = path.clone();
            drop(ss);
            if apply_via_restart_enabled() {
                schedule_process_restart();
                json_response(StatusCode::OK, r#"{"status":"restarting"}"#.to_owned())
            } else {
                let sub = state.subsystem.clone();
                tokio::spawn(async move {
                    match tokio::task::spawn_blocking(move || {
                        rt_updater::UpdateChecker::apply_and_exit(&path)
                    })
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            tracing::error!(error = %e, "update apply failed");
                            sub.lock().await.update_status = rt_updater::UpdateStatus::Failed {
                                error: e.to_string(),
                            };
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "update apply task failed");
                            sub.lock().await.update_status = rt_updater::UpdateStatus::Failed {
                                error: e.to_string(),
                            };
                        }
                    }
                });
                json_response(StatusCode::OK, r#"{"status":"applying"}"#.to_owned())
            }
        }
        None => {
            drop(ss);
            json_response(
                StatusCode::NOT_FOUND,
                r#"{"error":"no update staged"}"#.to_owned(),
            )
        }
    }
}

async fn update_check_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let update_mode = {
        let ss = state.subsystem.lock().await;
        ss.update_mode
    };

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
            state.subsystem.lock().await.update_status = status.clone();
            let body = serde_json::to_string(&status).unwrap_or_else(|_| {
                r#"{"status":"failed","error":"serialization error"}"#.to_owned()
            });
            return json_response(StatusCode::INTERNAL_SERVER_ERROR, body);
        }
    };

    let workflow_state =
        ForwarderWorkflowAdapter::new(state.subsystem.clone(), state.ui_tx.clone());
    let status = run_check(&workflow_state, &checker, update_mode).await;
    let body = serde_json::to_string(&status)
        .unwrap_or_else(|_| r#"{"status":"failed","error":"serialization error"}"#.to_owned());
    json_response(StatusCode::OK, body)
}

struct ForwarderWorkflowAdapter {
    subsystem: Arc<Mutex<SubsystemStatus>>,
    ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
}

impl ForwarderWorkflowAdapter {
    fn new(
        subsystem: Arc<Mutex<SubsystemStatus>>,
        ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
    ) -> Self {
        Self { subsystem, ui_tx }
    }
}

impl WorkflowState for ForwarderWorkflowAdapter {
    fn current_status<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = UpdateStatus> + Send + 'a>> {
        Box::pin(async move { self.subsystem.lock().await.update_status.clone() })
    }

    fn set_status<'a>(
        &'a self,
        status: UpdateStatus,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.subsystem.lock().await.update_status = status;
        })
    }

    fn set_downloaded<'a>(
        &'a self,
        status: UpdateStatus,
        path: std::path::PathBuf,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let mut ss = self.subsystem.lock().await;
            ss.update_status = status;
            ss.staged_update_path = Some(path);
        })
    }

    fn emit_status_changed<'a>(
        &'a self,
        status: UpdateStatus,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::UpdateStatusChanged { status });
        })
    }
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
            state.subsystem.lock().await.update_status = status.clone();
            let body = serde_json::to_string(&status).unwrap_or_else(|_| {
                r#"{"status":"failed","error":"serialization error"}"#.to_owned()
            });
            return json_response(StatusCode::INTERNAL_SERVER_ERROR, body);
        }
    };

    let workflow_state =
        ForwarderWorkflowAdapter::new(state.subsystem.clone(), state.ui_tx.clone());
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

    match read_config_json(&cs, &state.subsystem).await {
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

    match apply_section_update(
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

async fn post_config_server_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("server", state, body, None).await
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

async fn post_config_uplink_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("uplink", state, body, None).await
}

async fn post_config_status_http_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    post_config_section_handler("status_http", state, body, None).await
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
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
        assert_eq!(body["uplink_connected"], false);
        assert_eq!(body["restart_needed"], false);
        assert_eq!(body["readers"][0]["ip"], "192.168.1.10");
        assert_eq!(body["readers"][0]["state"], "connected");
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

        let mut rx = server.ui_tx.subscribe();
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

        let mut rx = server.ui_tx.subscribe();
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
        write!(
            config_file,
            r#"schema_version = 1
display_name = "Start Line"
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/fake-token"
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

        let mut rx = server.ui_tx.subscribe();
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
        write!(
            config_file,
            r#"schema_version = 1
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/fake-token"
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
            server.subsystem.lock().await.update_mode,
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
            server.subsystem.lock().await.update_mode,
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
        write!(
            config_file,
            r#"schema_version = 1
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/fake-token"
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
            server.subsystem.lock().await.update_mode,
            rt_updater::UpdateMode::CheckAndDownload
        );
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

        let workflow_state =
            ForwarderWorkflowAdapter::new(server.subsystem.clone(), server.ui_tx.clone());
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

        let workflow_state =
            ForwarderWorkflowAdapter::new(server.subsystem.clone(), server.ui_tx.clone());
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

        let workflow_state =
            ForwarderWorkflowAdapter::new(server.subsystem.clone(), server.ui_tx.clone());
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
            server.subsystem.lock().await.staged_update_path,
            Some(std::path::PathBuf::from("/tmp/staged-forwarder"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn power_action_execution_does_not_use_sudo_fallback() {
        let source = include_str!("status_http.rs");
        assert!(
            !source.contains("Command::new(\"sudo\")"),
            "power actions must not invoke sudo fallback"
        );
    }

    #[cfg(unix)]
    #[test]
    fn power_action_command_result_returns_500_on_spawn_error() {
        let result = map_power_action_command_result(
            "reboot",
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "systemctl not found",
            )),
            None,
        );

        let (status, body) = result.expect_err("spawn errors must return an HTTP error");
        assert_eq!(status, 500);
        assert!(body.contains("control action command failed"));
    }

    #[cfg(unix)]
    #[test]
    fn power_action_command_result_returns_500_on_non_zero_exit() {
        use std::os::unix::process::ExitStatusExt;

        let result = map_power_action_command_result(
            "poweroff",
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(1 << 8),
                stdout: vec![],
                stderr: vec![],
            }),
            None,
        );

        let (http_status, body) = result.expect_err("non-zero exit must return an HTTP error");
        assert_eq!(http_status, 500);
        assert!(body.contains("control action command exited with failure"));
    }

    #[cfg(unix)]
    #[test]
    fn power_action_command_result_returns_403_on_auth_failure() {
        use std::os::unix::process::ExitStatusExt;

        let result = map_power_action_command_result(
            "poweroff",
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(1 << 8),
                stdout: vec![],
                stderr: b"Call to PowerOff failed: Interactive authentication required.\n".to_vec(),
            }),
            None,
        );

        let (http_status, body) = result.expect_err("auth failures must return an HTTP error");
        assert_eq!(http_status, 403);
        assert!(
            body.to_ascii_lowercase()
                .contains("interactive authentication required")
        );
    }

    #[cfg(unix)]
    #[test]
    fn power_action_command_result_returns_500_on_non_auth_polkit_error() {
        use std::os::unix::process::ExitStatusExt;

        let result = map_power_action_command_result(
            "poweroff",
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(1 << 8),
                stdout: vec![],
                stderr: b"polkit daemon unavailable".to_vec(),
            }),
            None,
        );

        let (http_status, body) =
            result.expect_err("non-auth polkit failures must return an HTTP error");
        assert_eq!(http_status, 500);
        assert!(body.contains("polkit daemon unavailable"));
    }

    #[cfg(unix)]
    #[test]
    fn power_action_command_result_includes_stderr_in_error_body() {
        use std::os::unix::process::ExitStatusExt;

        let result = map_power_action_command_result(
            "reboot",
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(1 << 8),
                stdout: vec![],
                stderr: b"sudo: a password is required".to_vec(),
            }),
            None,
        );

        let (http_status, body) = result.expect_err("non-zero exit must return an HTTP error");
        assert_eq!(http_status, 403);
        assert!(body.contains("sudo: a password is required"));
    }

    #[cfg(unix)]
    #[test]
    fn power_action_command_result_returns_ok_on_success_exit() {
        use std::os::unix::process::ExitStatusExt;

        let result = map_power_action_command_result(
            "reboot",
            Ok(std::process::Output {
                status: std::process::ExitStatus::from_raw(0),
                stdout: vec![],
                stderr: vec![],
            }),
            None,
        );
        assert!(result.is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn power_action_join_error_logs_to_ui_when_logger_present() {
        let (tx, mut rx) = tokio::sync::broadcast::channel(16);
        let logger = rt_ui_log::UiLogger::new(tx, |entry| {
            crate::ui_events::ForwarderUiEvent::LogEntry { entry }
        });

        let join_err = tokio::task::spawn_blocking(|| -> () {
            panic!("boom");
        })
        .await
        .expect_err("task must panic");

        let (_status, _body) = map_power_action_join_error("reboot", join_err, Some(&logger));

        let evt = rx.try_recv().expect("expected UI log event");
        match evt {
            crate::ui_events::ForwarderUiEvent::LogEntry { entry } => {
                assert!(entry.contains("systemctl reboot task failed"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn control_action_from_config_forwards_logger_to_apply_fn() {
        let (tx, _) = tokio::sync::broadcast::channel(16);
        let logger = rt_ui_log::UiLogger::new(tx, |entry| {
            crate::ui_events::ForwarderUiEvent::LogEntry { entry }
        });
        let config = ConfigState::new(std::path::PathBuf::from("/tmp/unused.toml"));

        let saw_logger = Arc::new(AtomicBool::new(false));
        let spy = Arc::clone(&saw_logger);

        let _ = apply_control_action_from_config_with(
            "restart_device",
            &config,
            Some(&logger),
            move |_action, _config_state, _restart_signal, logger| {
                let spy = Arc::clone(&spy);
                let has_logger = logger.is_some();
                Box::pin(async move {
                    spy.store(has_logger, Ordering::SeqCst);
                    Err((500u16, "{\"ok\":false}".to_owned()))
                })
            },
        )
        .await;

        assert!(saw_logger.load(Ordering::SeqCst));
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

        let workflow_state =
            ForwarderWorkflowAdapter::new(server.subsystem.clone(), server.ui_tx.clone());
        let status = run_download(&workflow_state, &checker).await;

        assert_eq!(
            status,
            Ok(UpdateStatus::Downloaded {
                version: "2.0.0".to_owned()
            })
        );
        assert_eq!(download_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            server.subsystem.lock().await.staged_update_path,
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
        let mut rx = server.ui_tx.subscribe();

        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Err("boom".to_owned()),
            download_calls: Arc::new(AtomicUsize::new(0)),
        };

        let workflow_state =
            ForwarderWorkflowAdapter::new(server.subsystem.clone(), server.ui_tx.clone());
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

        let workflow_state =
            ForwarderWorkflowAdapter::new(server.subsystem.clone(), server.ui_tx.clone());
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

        let workflow_state =
            ForwarderWorkflowAdapter::new(server.subsystem.clone(), server.ui_tx.clone());
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

        let mut rx = server.ui_tx.subscribe();

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

        let mut rx = server.ui_tx.subscribe();

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
}
