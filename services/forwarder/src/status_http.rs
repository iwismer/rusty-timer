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

use crate::storage::journal::Journal;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use rt_updater::UpdateStatus;
use rt_updater::workflow::{RealChecker, WorkflowState, run_check, run_download};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::future::Future;
use std::io::Write as _;
use std::net::{SocketAddr, SocketAddrV4};
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
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
// Subsystem readiness
// ---------------------------------------------------------------------------

/// Connection state of a reader TCP socket.
#[derive(Debug, Clone, PartialEq)]
pub enum ReaderConnectionState {
    Connecting,
    Connected,
    Disconnected,
}

impl From<&ReaderConnectionState> for crate::ui_events::ReaderConnectionState {
    fn from(state: &ReaderConnectionState) -> Self {
        match state {
            ReaderConnectionState::Connected => crate::ui_events::ReaderConnectionState::Connected,
            ReaderConnectionState::Connecting => {
                crate::ui_events::ReaderConnectionState::Connecting
            }
            ReaderConnectionState::Disconnected => {
                crate::ui_events::ReaderConnectionState::Disconnected
            }
        }
    }
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
    /// The name of the current epoch, if any.
    pub current_epoch_name: Option<String>,
    /// Control protocol info (firmware, clock, etc.) — populated on connect.
    pub reader_info: Option<crate::reader_control::ReaderInfo>,
}

/// UPS daemon availability + latest readings snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpsStatusState {
    pub available: bool,
    pub status: Option<rt_domain::UpsStatus>,
}

/// Forwarder-local status updates consumed by P2P control sessions.
#[derive(Debug, Clone)]
pub enum ForwarderStatusEvent {
    ReaderStatus {
        stream_id: String,
        status: ReaderStatus,
    },
    ReaderInfo {
        stream_id: String,
        info: crate::reader_control::ReaderInfo,
    },
    DownloadProgress {
        stream_id: String,
        event: crate::reader_control::DownloadEvent,
    },
    UpsStatus(UpsStatusState),
}

/// Current forwarder status snapshot consumed by a newly connected P2P peer.
#[derive(Debug, Clone, Default)]
pub struct ForwarderStatusSnapshot {
    pub readers: Vec<(String, ReaderStatus)>,
    pub ups_status: Option<UpsStatusState>,
}

/// Read-only status feed for P2P control sessions.
#[derive(Clone)]
pub struct ForwarderStatusFeed {
    subsystem: Arc<Mutex<SubsystemStatus>>,
    status_event_tx: broadcast::Sender<ForwarderStatusEvent>,
}

impl ForwarderStatusFeed {
    /// Atomically subscribe to the status broadcast and capture the current
    /// snapshot under the same `SubsystemStatus` lock.
    ///
    /// Holding the lock across both operations guarantees the returned snapshot
    /// and the delta stream do not overlap: every status update either landed in
    /// the snapshot (and was broadcast before this subscription existed) or is
    /// delivered as a post-snapshot delta on the returned receiver. This relies
    /// on all `ForwarderStatusEvent` broadcasts being emitted while holding the
    /// same lock. No `.await` is held across the lock.
    pub async fn subscribe_and_snapshot(
        &self,
    ) -> (
        broadcast::Receiver<ForwarderStatusEvent>,
        ForwarderStatusSnapshot,
    ) {
        let ss = self.subsystem.lock().await;
        let receiver = self.status_event_tx.subscribe();
        let snapshot = ForwarderStatusSnapshot {
            readers: ss
                .readers()
                .iter()
                .map(|(stream_id, status)| (stream_id.clone(), status.clone()))
                .collect(),
            ups_status: ss.ups_status().cloned(),
        };
        (receiver, snapshot)
    }
}

/// Tracks local subsystem readiness for the `/readyz` endpoint.
///
/// Ready = config loaded + journal open + worker tasks started.
/// P2P session connectivity is explicitly excluded from readiness.
#[derive(Debug, Clone)]
pub struct SubsystemStatus {
    ready: bool,
    reason: Option<String>,
    /// P2P session state is tracked for the status page but does NOT affect readiness.
    p2p_connected: bool,
    p2p_endpoint_id: Option<String>,
    forwarder_id: String,
    local_ip: Option<String>,
    pub(crate) readers: HashMap<String, ReaderStatus>,
    update_status: UpdateStatus,
    staged_update_path: Option<std::path::PathBuf>,
    pub update_mode: rt_updater::UpdateMode,
    /// Set to `true` when config is saved and the forwarder needs a restart to apply changes.
    restart_needed: bool,
    /// UPS status snapshot (None if UPS monitoring is not configured).
    ups_status: Option<UpsStatusState>,
    /// Readers whose read counters changed since the last coalesced P2P
    /// status broadcast (see [`spawn_read_count_broadcaster`]).
    read_counts_dirty: HashSet<String>,
}

impl SubsystemStatus {
    /// Create a fully-ready subsystem status.
    pub fn ready() -> Self {
        SubsystemStatus {
            ready: true,
            reason: None,
            p2p_connected: false,
            p2p_endpoint_id: None,
            forwarder_id: String::new(),
            local_ip: None,
            readers: HashMap::new(),
            update_status: UpdateStatus::UpToDate,
            staged_update_path: None,
            update_mode: rt_updater::UpdateMode::default(),
            restart_needed: false,
            ups_status: None,
            read_counts_dirty: HashSet::new(),
        }
    }

    /// Create a not-ready subsystem status with a reason.
    pub fn not_ready(reason: String) -> Self {
        SubsystemStatus {
            ready: false,
            reason: Some(reason),
            p2p_connected: false,
            p2p_endpoint_id: None,
            forwarder_id: String::new(),
            local_ip: None,
            readers: HashMap::new(),
            update_status: UpdateStatus::UpToDate,
            staged_update_path: None,
            update_mode: rt_updater::UpdateMode::default(),
            restart_needed: false,
            ups_status: None,
            read_counts_dirty: HashSet::new(),
        }
    }

    /// Set the P2P session state (does NOT affect `/readyz` result).
    pub fn set_p2p_connected(&mut self, connected: bool) {
        self.p2p_connected = connected;
    }

    /// Return true if all local subsystems are ready.
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Return the P2P session state.
    pub fn p2p_connected(&self) -> bool {
        self.p2p_connected
    }

    pub fn set_p2p_endpoint_id(&mut self, endpoint_id: String) {
        self.p2p_endpoint_id = Some(endpoint_id);
    }

    /// Return whether a restart is needed to apply saved config changes.
    pub fn restart_needed(&self) -> bool {
        self.restart_needed
    }

    /// Mark that a restart is needed to apply saved config changes.
    pub fn set_restart_needed(&mut self) {
        self.restart_needed = true;
    }

    /// Return the connection state for a given reader IP, if tracked.
    pub fn reader_connection_state(&self, reader_ip: &str) -> Option<ReaderConnectionState> {
        self.readers.get(reader_ip).map(|r| r.state.clone())
    }

    /// Return a reference to the readers map.
    pub fn readers(&self) -> &HashMap<String, ReaderStatus> {
        &self.readers
    }

    pub(crate) fn cached_reader_info(
        &self,
        reader_ip: &str,
    ) -> Option<crate::reader_control::ReaderInfo> {
        self.readers
            .get(reader_ip)
            .and_then(|r| r.reader_info.clone())
    }

    pub(crate) fn update_cached_reader_info_unless_disconnected(
        &mut self,
        reader_ip: &str,
        info: crate::reader_control::ReaderInfo,
    ) -> bool {
        let Some(reader) = self.readers.get_mut(reader_ip) else {
            tracing::warn!(reader_ip = %reader_ip, "update_cached_reader_info: reader not found in status map, skipping broadcast");
            return false;
        };
        if reader.state == ReaderConnectionState::Disconnected {
            tracing::debug!(
                reader_ip,
                "dropping cached reader info update for disconnected reader"
            );
            return false;
        }
        reader.reader_info = Some(info);
        true
    }

    /// Set the UPS status snapshot.
    pub fn set_ups_status(&mut self, state: UpsStatusState) {
        self.ups_status = Some(state);
    }

    /// Return the current UPS status snapshot, if any.
    pub fn ups_status(&self) -> Option<&UpsStatusState> {
        self.ups_status.as_ref()
    }
}

#[cfg(any(feature = "eink", feature = "lcd"))]
fn subsystem_to_display_state(
    ss: &SubsystemStatus,
    forwarder_name: Option<String>,
    cpu_temp: Option<f32>,
) -> rt_screen::state::DisplayState {
    let readers = ss
        .readers()
        .iter()
        .map(|(addr, r)| {
            // Reader addresses are "ip:port" — extract just the IP.
            let ip = addr
                .rsplit_once(':')
                .map_or(addr.as_str(), |(ip, _)| ip)
                .to_owned();
            rt_screen::state::ReaderDisplayState {
                ip,
                state: match r.state {
                    ReaderConnectionState::Connected => {
                        rt_screen::state::ReaderConnectionState::Connected
                    }
                    ReaderConnectionState::Connecting => {
                        rt_screen::state::ReaderConnectionState::Connecting
                    }
                    ReaderConnectionState::Disconnected => {
                        rt_screen::state::ReaderConnectionState::Disconnected
                    }
                },
                drift_ms: r
                    .reader_info
                    .as_ref()
                    .and_then(|info| info.clock.as_ref())
                    .map(|c| c.drift_ms),
                session_reads: r.reads_since_restart,
            }
        })
        .collect();

    let total_reads: u64 = ss.readers().values().map(|r| r.reads_since_restart).sum();

    rt_screen::state::DisplayState {
        forwarder_name,
        local_ip: ss.local_ip.clone(),
        p2p_connected: ss.p2p_connected(),
        readers,
        total_reads,
        cpu_temp_celsius: cpu_temp,
        battery: ss.ups_status().and_then(|u| {
            u.status.as_ref().map(|s| rt_screen::state::BatteryState {
                percent: s.battery_percent,
                charging: s.charging,
            })
        }),
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
    #[cfg(any(feature = "eink", feature = "lcd"))]
    display_tx: Option<tokio::sync::watch::Sender<rt_screen::state::DisplayState>>,
    #[cfg(any(feature = "eink", feature = "lcd"))]
    display_name: Arc<Mutex<Option<String>>>,
    #[cfg(any(feature = "eink", feature = "lcd"))]
    cpu_temp: Arc<Mutex<Option<f32>>>,
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
    #[cfg(any(feature = "eink", feature = "lcd"))]
    display_name: Arc<Mutex<Option<String>>>,
    #[cfg(any(feature = "eink", feature = "lcd"))]
    cpu_temp: Arc<Mutex<Option<f32>>>,
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
            #[cfg(any(feature = "eink", feature = "lcd"))]
            display_name: self.display_name.clone(),
            #[cfg(any(feature = "eink", feature = "lcd"))]
            cpu_temp: self.cpu_temp.clone(),
        }
    }
}

fn bridge_download_progress_events(
    stream_id: String,
    tracker: Arc<tokio::sync::Mutex<crate::reader_control::DownloadTracker>>,
    status_event_tx: broadcast::Sender<ForwarderStatusEvent>,
) {
    if let Ok(tracker) = tracker.try_lock() {
        spawn_download_progress_bridge(stream_id, tracker.subscribe(), status_event_tx);
        return;
    }

    tokio::spawn(async move {
        let rx = {
            let tracker = tracker.lock().await;
            tracker.subscribe()
        };
        spawn_download_progress_bridge(stream_id, rx, status_event_tx);
    });
}

fn spawn_download_progress_bridge(
    stream_id: String,
    mut rx: broadcast::Receiver<crate::reader_control::DownloadEvent>,
    status_event_tx: broadcast::Sender<ForwarderStatusEvent>,
) {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    let _ = status_event_tx.send(ForwarderStatusEvent::DownloadProgress {
                        stream_id: stream_id.clone(),
                        event,
                    });
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!(skipped = n, "download status bridge lagged");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

/// Interval at which accumulated read-count changes are broadcast to P2P
/// control sessions as `ReaderStatus` deltas.
const READ_COUNT_BROADCAST_INTERVAL: Duration = Duration::from_secs(2);

/// Broadcast a `ReaderStatus` delta for every reader whose read counters
/// changed since the last tick (see `record_read`).
///
/// Sends while holding the `SubsystemStatus` lock so deltas stay ordered
/// against `subscribe_and_snapshot`, like every other status broadcast.
/// Broadcasting current state (not increments) makes a dropped or duplicated
/// delta harmless: the next tick carries the up-to-date counters.
async fn broadcast_dirty_read_counts(
    subsystem: &Mutex<SubsystemStatus>,
    status_event_tx: &broadcast::Sender<ForwarderStatusEvent>,
) {
    let mut ss = subsystem.lock().await;
    if ss.read_counts_dirty.is_empty() {
        return;
    }
    let dirty = std::mem::take(&mut ss.read_counts_dirty);
    for reader_ip in dirty {
        if let Some(status) = ss.readers.get(&reader_ip) {
            let _ = status_event_tx.send(ForwarderStatusEvent::ReaderStatus {
                stream_id: reader_ip,
                status: status.clone(),
            });
        }
    }
}

/// Spawn the coalescing task that pushes read-count updates to P2P peers at a
/// bounded rate. Quiet when no reads arrive; at most one delta per reader per
/// interval during bursts.
fn spawn_read_count_broadcaster(
    subsystem: Arc<Mutex<SubsystemStatus>>,
    status_event_tx: broadcast::Sender<ForwarderStatusEvent>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(READ_COUNT_BROADCAST_INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            broadcast_dirty_read_counts(&subsystem, &status_event_tx).await;
        }
    });
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

    /// Return a read-only status feed for P2P control sessions.
    pub fn status_feed(&self) -> ForwarderStatusFeed {
        ForwarderStatusFeed {
            subsystem: self.subsystem.clone(),
            status_event_tx: self.status_event_tx.clone(),
        }
    }

    /// Return the shared reader-control service used by HTTP and P2P control paths.
    pub fn reader_control_service(&self) -> crate::reader_control_service::ReaderControlService {
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

    /// Return a clone of the shared UI logger Arc.
    pub fn logger(&self) -> Arc<rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>> {
        self.logger.clone()
    }

    /// Mark all local subsystems as ready.
    pub async fn set_ready(&self) {
        {
            let mut ss = self.subsystem.lock().await;
            ss.ready = true;
            ss.reason = None;
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::StatusChanged {
                    ready: ss.is_ready(),
                    p2p_connected: ss.p2p_connected(),
                    restart_needed: ss.restart_needed(),
                });
        }
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
    }

    /// Mark that a restart is needed to apply saved config changes.
    pub async fn set_restart_needed(&self) {
        mark_restart_needed_and_emit(&self.subsystem, &self.ui_tx).await;
    }

    /// Return whether a restart is needed to apply saved config changes.
    pub async fn restart_needed(&self) -> bool {
        self.subsystem.lock().await.restart_needed()
    }

    pub async fn set_p2p_endpoint_id(&self, endpoint_id: String) {
        let mut ss = self.subsystem.lock().await;
        ss.set_p2p_endpoint_id(endpoint_id);
    }

    /// Update the P2P session state (does not affect readiness).
    pub async fn set_p2p_connected(&self, connected: bool) {
        {
            let mut ss = self.subsystem.lock().await;
            ss.set_p2p_connected(connected);
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::StatusChanged {
                    ready: ss.is_ready(),
                    p2p_connected: connected,
                    restart_needed: ss.restart_needed(),
                });
        }
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
    }

    /// Set the forwarder ID (call once at startup).
    pub async fn set_forwarder_id(&self, id: &str) {
        self.subsystem.lock().await.forwarder_id = id.to_owned();
    }

    /// Set the detected local IP (at startup and on reader connect/disconnect).
    pub async fn set_local_ip(&self, ip: Option<String>) {
        self.subsystem.lock().await.local_ip = ip;
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
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

    /// Update the UPS status snapshot in the subsystem state.
    ///
    /// The local HTTP/UI snapshot is always updated, but the P2P control-event
    /// broadcast only fires when the UPS status actually changed from the stored
    /// previous value. The UPS poller calls this every interval, so gating the
    /// broadcast on a real change avoids spamming connected receivers with
    /// identical `UpsStatus` frames. The send happens under the subsystem lock so
    /// it is ordered against `subscribe_and_snapshot`.
    pub async fn set_ups_status(&self, state: UpsStatusState) {
        {
            let mut ss = self.subsystem.lock().await;
            let changed = ss.ups_status() != Some(&state);
            ss.set_ups_status(state.clone());
            if changed {
                let _ = self
                    .status_event_tx
                    .send(ForwarderStatusEvent::UpsStatus(state));
            }
        }
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
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
            .insert(reader_ip.to_owned(), tracker.clone());
        bridge_download_progress_events(
            reader_ip.to_owned(),
            tracker,
            self.status_event_tx.clone(),
        );
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

    #[cfg(any(feature = "eink", feature = "lcd"))]
    async fn publish_display_state(&self) {
        if let Some(ref tx) = self.display_tx {
            let ss = self.subsystem.lock().await;
            let forwarder_name = self.display_name.lock().await.clone();
            let cpu_temp = *self.cpu_temp.lock().await;
            let state = subsystem_to_display_state(&ss, forwarder_name, cpu_temp);
            tx.send_replace(state);
        }
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub fn set_display_sender(
        &mut self,
        tx: tokio::sync::watch::Sender<rt_screen::state::DisplayState>,
    ) {
        self.display_tx = Some(tx);
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub async fn set_display_name(&self, name: Option<String>) {
        *self.display_name.lock().await = name;
        self.publish_display_state().await;
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub async fn set_cpu_temp(&self, temp: Option<f32>) {
        self.set_cpu_temp_cached(temp).await;
        self.publish_display_state().await;
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub async fn set_cpu_temp_cached(&self, temp: Option<f32>) {
        *self.cpu_temp.lock().await = temp;
    }

    /// Retrieve a clone of the cached reader info for a given reader IP.
    pub async fn get_reader_info(
        &self,
        reader_ip: &str,
    ) -> Option<crate::reader_control::ReaderInfo> {
        let ss = self.subsystem.lock().await;
        ss.readers
            .get(reader_ip)
            .and_then(|r| r.reader_info.clone())
    }

    pub async fn update_reader_info(
        &self,
        reader_ip: &str,
        info: crate::reader_control::ReaderInfo,
    ) {
        {
            let mut ss = self.subsystem.lock().await;
            if let Some(r) = ss.readers.get_mut(reader_ip) {
                r.reader_info = Some(info.clone());
            }
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ReaderInfoUpdated {
                    ip: reader_ip.to_owned(),
                    info: info.clone(),
                });
            // Broadcast under the lock so it is ordered against
            // `subscribe_and_snapshot`.
            let _ = self.status_event_tx.send(ForwarderStatusEvent::ReaderInfo {
                stream_id: reader_ip.to_owned(),
                info,
            });
        }
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
    }

    /// Update reader info only if the reader has not transitioned to Disconnected.
    ///
    /// This is used by the background poller so a late poll result cannot restore
    /// stale info after the read loop has already marked the reader disconnected.
    pub async fn update_reader_info_unless_disconnected(
        &self,
        reader_ip: &str,
        info: crate::reader_control::ReaderInfo,
    ) {
        {
            let mut ss = self.subsystem.lock().await;
            if let Some(r) = ss.readers.get_mut(reader_ip) {
                if r.state == ReaderConnectionState::Disconnected {
                    tracing::debug!(
                        reader_ip,
                        "dropping reader info update for disconnected reader"
                    );
                    return;
                }
                r.reader_info = Some(info.clone());
            } else {
                tracing::debug!(
                    reader_ip,
                    "reader IP not found in map, skipping info update"
                );
                return;
            }
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ReaderInfoUpdated {
                    ip: reader_ip.to_owned(),
                    info: info.clone(),
                });
            // Broadcast under the lock so it is ordered against
            // `subscribe_and_snapshot`.
            let _ = self.status_event_tx.send(ForwarderStatusEvent::ReaderInfo {
                stream_id: reader_ip.to_owned(),
                info,
            });
        }
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
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
        {
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
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
    }

    /// Seed a reader's total historical count from durable journal state.
    pub async fn set_reader_total(&self, reader_ip: &str, total: i64) {
        {
            let mut ss = self.subsystem.lock().await;
            if let Some(r) = ss.readers.get_mut(reader_ip) {
                r.reads_total = total;
            }
        }
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
    }

    /// Set the current epoch name for a reader and broadcast a ReaderUpdated SSE event.
    pub async fn set_reader_epoch_name(&self, reader_ip: &str, name: Option<String>) {
        {
            let mut ss = self.subsystem.lock().await;
            if let Some(r) = ss.readers.get_mut(reader_ip) {
                r.current_epoch_name = name;
                let _ = self
                    .ui_tx
                    .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                        ip: reader_ip.to_owned(),
                        state: (&r.state).into(),
                        reads_session: r.reads_since_restart,
                        reads_total: r.reads_total,
                        last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                        local_port: r.local_port,
                        current_epoch_name: r.current_epoch_name.clone(),
                    });
            }
        }
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
    }

    /// Update a reader's connection state.
    pub async fn update_reader_state(&self, reader_ip: &str, state: ReaderConnectionState) {
        {
            let mut ss = self.subsystem.lock().await;
            if let Some(r) = ss.readers.get_mut(reader_ip) {
                let changed = r.state != state;
                if state == ReaderConnectionState::Disconnected {
                    r.reader_info = None;
                }
                r.state = state;
                let _ = self
                    .ui_tx
                    .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                        ip: reader_ip.to_owned(),
                        state: (&r.state).into(),
                        reads_session: r.reads_since_restart,
                        reads_total: r.reads_total,
                        last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                        local_port: r.local_port,
                        current_epoch_name: r.current_epoch_name.clone(),
                    });
                // Only broadcast a P2P control delta on an actual state
                // transition, and do so under the lock so it is ordered against
                // `subscribe_and_snapshot`. The local UI event above is always
                // emitted.
                if changed {
                    let _ = self
                        .status_event_tx
                        .send(ForwarderStatusEvent::ReaderStatus {
                            stream_id: reader_ip.to_owned(),
                            status: r.clone(),
                        });
                }
            }
        }
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
    }

    /// Record a successful chip read for a reader.
    pub async fn record_read(&self, reader_ip: &str) {
        {
            let mut ss = self.subsystem.lock().await;
            if let Some(r) = ss.readers.get_mut(reader_ip) {
                r.reads_since_restart += 1;
                r.reads_total += 1;
                r.last_seen = Some(Instant::now());
                let _ = self
                    .ui_tx
                    .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                        ip: reader_ip.to_owned(),
                        state: (&r.state).into(),
                        reads_session: r.reads_since_restart,
                        reads_total: r.reads_total,
                        last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                        local_port: r.local_port,
                        current_epoch_name: r.current_epoch_name.clone(),
                    });
                // Per-read P2P broadcasts would flood the control stream during
                // bursts, so reads only mark the reader dirty; the coalescing
                // task broadcasts the latest counters at a bounded rate.
                ss.read_counts_dirty.insert(reader_ip.to_owned());
            }
        }
        #[cfg(any(feature = "eink", feature = "lcd"))]
        self.publish_display_state().await;
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
        let (status_event_tx, _) = broadcast::channel(256);
        let logger = Arc::new(rt_ui_log::UiLogger::with_buffer(
            ui_tx.clone(),
            |entry| crate::ui_events::ForwarderUiEvent::LogEntry { entry },
            500,
        ));
        let subsystem = Arc::new(Mutex::new(subsystem));
        let control_clients = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let download_trackers = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let reconnect_notifies = Arc::new(std::sync::RwLock::new(HashMap::new()));
        #[cfg(any(feature = "eink", feature = "lcd"))]
        let display_name = Arc::new(Mutex::new(None));
        #[cfg(any(feature = "eink", feature = "lcd"))]
        let cpu_temp = Arc::new(Mutex::new(None));
        let state = AppState {
            subsystem: subsystem.clone(),
            journal,
            version: Arc::new(cfg.forwarder_version),
            config_state: None,
            restart_signal: None,
            ui_tx: ui_tx.clone(),
            status_event_tx: status_event_tx.clone(),
            logger: logger.clone(),
            control_clients: control_clients.clone(),
            download_trackers: download_trackers.clone(),
            reconnect_notifies: reconnect_notifies.clone(),
            #[cfg(any(feature = "eink", feature = "lcd"))]
            display_name: display_name.clone(),
            #[cfg(any(feature = "eink", feature = "lcd"))]
            cpu_temp: cpu_temp.clone(),
        };

        let app = build_router(state);
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                tracing::error!(error = %err, "status HTTP server fatal error");
            }
        });
        spawn_read_count_broadcaster(subsystem.clone(), status_event_tx.clone());

        Ok(StatusServer {
            local_addr,
            subsystem,
            ui_tx,
            status_event_tx,
            logger,
            control_clients,
            download_trackers,
            reconnect_notifies,
            #[cfg(any(feature = "eink", feature = "lcd"))]
            display_tx: None,
            #[cfg(any(feature = "eink", feature = "lcd"))]
            display_name,
            #[cfg(any(feature = "eink", feature = "lcd"))]
            cpu_temp,
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
        let (status_event_tx, _) = broadcast::channel(256);
        let logger = Arc::new(rt_ui_log::UiLogger::with_buffer(
            ui_tx.clone(),
            |entry| crate::ui_events::ForwarderUiEvent::LogEntry { entry },
            500,
        ));
        let subsystem = Arc::new(Mutex::new(subsystem));
        let control_clients = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let download_trackers = Arc::new(std::sync::RwLock::new(HashMap::new()));
        let reconnect_notifies = Arc::new(std::sync::RwLock::new(HashMap::new()));
        #[cfg(any(feature = "eink", feature = "lcd"))]
        let display_name = Arc::new(Mutex::new(None));
        #[cfg(any(feature = "eink", feature = "lcd"))]
        let cpu_temp = Arc::new(Mutex::new(None));
        let state = AppState {
            subsystem: subsystem.clone(),
            journal,
            version: Arc::new(cfg.forwarder_version),
            config_state: Some(config_state),
            restart_signal: Some(restart_signal),
            ui_tx: ui_tx.clone(),
            status_event_tx: status_event_tx.clone(),
            logger: logger.clone(),
            control_clients: control_clients.clone(),
            download_trackers: download_trackers.clone(),
            reconnect_notifies: reconnect_notifies.clone(),
            #[cfg(any(feature = "eink", feature = "lcd"))]
            display_name: display_name.clone(),
            #[cfg(any(feature = "eink", feature = "lcd"))]
            cpu_temp: cpu_temp.clone(),
        };

        let app = build_router(state);
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                tracing::error!(error = %err, "status HTTP server fatal error");
            }
        });
        spawn_read_count_broadcaster(subsystem.clone(), status_event_tx.clone());

        Ok(StatusServer {
            local_addr,
            subsystem,
            ui_tx,
            status_event_tx,
            logger,
            control_clients,
            download_trackers,
            reconnect_notifies,
            #[cfg(any(feature = "eink", feature = "lcd"))]
            display_tx: None,
            #[cfg(any(feature = "eink", feature = "lcd"))]
            display_name,
            #[cfg(any(feature = "eink", feature = "lcd"))]
            cpu_temp,
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

    crate::config::load_config_from_str(&new_toml, &config_state.path).map_err(|e| {
        (
            400u16,
            serde_json::json!({"ok": false, "error": format!("config validation failed: {e}")})
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
        p2p_connected: ss.p2p_connected(),
        restart_needed: true,
    });
}

/// Apply a config section update by name.
///
/// Dispatches to the right mutation logic based on `section`, validates the
/// payload, and calls `update_config_file` to persist the change.
///
/// Recognised sections: `"general"`, `"auth"`, `"journal"`, `"status_http"`,
/// `"control"`, `"update"`, `"p2p"`, `"ups"`, `"readers"`, and `"screen"`.
/// Screen config changes require a restart to apply.
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
            let min_retention = optional_string_field(payload, "min_retention")?;
            let max_retention = optional_string_field(payload, "max_retention")?;
            let emergency_free_disk_bytes =
                optional_u64_field(payload, "emergency_free_disk_bytes")?;
            let emergency_max_rows = optional_u64_field(payload, "emergency_max_rows")?
                .map(|value| {
                    i64::try_from(value)
                        .map_err(|_| bad_request_error("emergency_max_rows must be <= i64::MAX"))
                })
                .transpose()?;
            if let Some(pct) = prune_watermark_pct
                && pct > 100
            {
                return Err(bad_request_error(
                    "prune_watermark_pct must be between 0 and 100",
                ));
            }
            crate::config::validate_retention_settings(
                min_retention.as_deref(),
                max_retention.as_deref(),
                emergency_max_rows,
            )
            .map_err(bad_request_error)?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.journal = Some(crate::config::RawJournalConfig {
                    sqlite_path,
                    prune_watermark_pct,
                    min_retention,
                    max_retention,
                    emergency_free_disk_bytes,
                    emergency_max_rows,
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
                // Preserve existing P2P control settings; this handler only
                // mutates allow_power_actions.
                let allow_remote_config = raw.control.as_ref().and_then(|c| c.allow_remote_config);
                let allow_reader_control =
                    raw.control.as_ref().and_then(|c| c.allow_reader_control);
                raw.control = Some(crate::config::RawControlConfig {
                    allow_power_actions,
                    allow_remote_config,
                    allow_reader_control,
                });
                Ok(())
            })
            .await
        }
        "p2p" => {
            let enabled = optional_bool_field(payload, "enabled")?;
            let server_url = optional_string_field(payload, "server_url")?.and_then(|url| {
                let trimmed = url.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            });
            let server_token_file =
                optional_string_field(payload, "server_token_file")?.and_then(|path| {
                    let trimmed = path.trim();
                    (!trimmed.is_empty()).then(|| trimmed.to_owned())
                });
            if let Some(ref url) = server_url {
                validate_server_url(url).map_err(bad_request_error)?;
            }
            if let Some(ref token_file) = server_token_file {
                validate_token_file(token_file).map_err(bad_request_error)?;
            }
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                let previous = raw.p2p.take();
                raw.p2p = Some(crate::config::RawP2pConfig {
                    enabled,
                    secret_key_path: previous
                        .as_ref()
                        .and_then(|cfg| cfg.secret_key_path.clone()),
                    secret_key_seed_hex: previous
                        .as_ref()
                        .and_then(|cfg| cfg.secret_key_seed_hex.clone()),
                    bind_addr_v4: previous.as_ref().and_then(|cfg| cfg.bind_addr_v4.clone()),
                    relay_disabled: previous.as_ref().and_then(|cfg| cfg.relay_disabled),
                    discovery_disabled: previous.as_ref().and_then(|cfg| cfg.discovery_disabled),
                    max_concurrent_bidi_streams: previous
                        .as_ref()
                        .and_then(|cfg| cfg.max_concurrent_bidi_streams),
                    static_allowed_receivers: previous
                        .as_ref()
                        .and_then(|cfg| cfg.static_allowed_receivers.clone()),
                    allowlist_cache_path: previous
                        .as_ref()
                        .and_then(|cfg| cfg.allowlist_cache_path.clone()),
                    server_url,
                    server_token_file,
                    device_token_file: previous
                        .as_ref()
                        .and_then(|cfg| cfg.device_token_file.clone()),
                    allowlist_poll_interval_secs: previous
                        .as_ref()
                        .and_then(|cfg| cfg.allowlist_poll_interval_secs),
                    allowlist_request_timeout_secs: previous
                        .as_ref()
                        .and_then(|cfg| cfg.allowlist_request_timeout_secs),
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
        "ups" => {
            let enabled = optional_bool_field(payload, "enabled")?;
            let daemon_addr = optional_string_field(payload, "daemon_addr")?.and_then(|addr| {
                let trimmed = addr.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_owned())
            });
            let poll_interval_secs = optional_u64_field(payload, "poll_interval_secs")?;
            let upstream_heartbeat_secs = optional_u64_field(payload, "upstream_heartbeat_secs")?;

            if let Some(interval) = poll_interval_secs
                && !(1..=60).contains(&interval)
            {
                return Err(bad_request_error(
                    "poll_interval_secs must be between 1 and 60",
                ));
            }
            if let Some(heartbeat) = upstream_heartbeat_secs
                && !(10..=300).contains(&heartbeat)
            {
                return Err(bad_request_error(
                    "upstream_heartbeat_secs must be between 10 and 300",
                ));
            }
            if let Some(ref addr) = daemon_addr
                && addr.parse::<std::net::SocketAddr>().is_err()
            {
                let parts: Vec<&str> = addr.rsplitn(2, ':').collect();
                if parts.len() != 2 || parts[0].parse::<u16>().is_err() {
                    return Err(bad_request_error(format!(
                        "daemon_addr must be a valid host:port, got '{}'",
                        addr
                    )));
                }
            }

            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.ups = Some(crate::config::RawUpsConfig {
                    enabled,
                    daemon_addr,
                    poll_interval_secs,
                    upstream_heartbeat_secs,
                });
                Ok(())
            })
            .await
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
        #[cfg(any(feature = "eink", feature = "lcd"))]
        "screen" => {
            let parsed = serde_json::from_value::<rt_screen::state::ScreenConfig>(payload.clone())
                .map_err(|e| bad_request_error(e.to_string()))?;
            // Validate before persisting so the endpoint can't accept values the
            // loader would later reject (which would make the forwarder fail to
            // boot on the very restart this change requests).
            crate::config::validate_screen_config(&parsed)
                .map_err(|e| bad_request_error(e.to_string()))?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.screen = Some(parsed);
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

/// Read the config TOML file as a JSON value plus the current `restart_needed`
/// state, returning a plain error message on failure.
///
/// Shared core for both the HTTP `GET /api/v1/config` endpoint and the P2P
/// remote-config get verb, so the two always agree on the serialized shape.
async fn read_config_value(
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
) -> Result<(serde_json::Value, bool), String> {
    let _lock = config_state.write_lock.lock().await;

    let toml_str =
        std::fs::read_to_string(&config_state.path).map_err(|e| format!("File read error: {e}"))?;

    let raw: crate::config::RawConfig =
        toml::from_str(&toml_str).map_err(|e| format!("TOML parse error: {e}"))?;

    let json = serde_json::to_value(&raw).map_err(|e| format!("JSON serialize error: {e}"))?;

    let restart_needed = subsystem.lock().await.restart_needed();
    Ok((json, restart_needed))
}

/// Read the config TOML file as JSON.
///
/// Returns `(config_json, restart_needed)` on success.
pub async fn read_config_json(
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
) -> Result<(serde_json::Value, bool), (u16, String)> {
    read_config_value(config_state, subsystem)
        .await
        .map_err(|e| {
            (
                500u16,
                serde_json::json!({"ok": false, "error": e}).to_string(),
            )
        })
}

/// Serialize the current config to a JSON string (identical to the body
/// `GET /api/v1/config` returns) plus the current `restart_needed` state.
///
/// Used by the P2P remote-config get verb so the receiver UI round-trips the
/// same document whether it reads config over HTTP or P2P.
pub async fn config_json_string(
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
) -> Result<(String, bool), String> {
    let (value, restart_needed) = read_config_value(config_state, subsystem).await?;
    let json = serde_json::to_string(&value).map_err(|e| format!("JSON serialize error: {e}"))?;
    Ok((json, restart_needed))
}

/// Sections a P2P peer may never change: identity/credentials (`auth`),
/// transport trust (`p2p`), and the gates themselves (`control`).
///
/// Boundary rationale: `[auth]`/`[p2p]`/`[control]` changes are trust/identity
/// escalation — permanent and self-granting; `[journal]`, `[[readers]]`, and
/// display fields are the operational surface remote management exists to serve
/// — a hostile allow-listed receiver can disrupt operations through them but
/// cannot expand its own access. `[status_http]`, `[update]`, and `[ups]` were
/// considered and left writable as operational surface; `status_http.bind` is
/// the borderline case (a receiver could rebind the local admin HTTP), but it
/// grants the receiver itself no access.
const REMOTE_PROTECTED_SECTIONS: &[&str] = &["auth", "p2p", "control"];

/// Persist a full config document received from P2P remote config, rejecting
/// any attempted change to privileged config sections before writing.
///
/// The document is parsed into the same [`crate::config::RawConfig`] the get
/// path serializes, re-serialized to TOML, and validated by running the
/// canonical loader before anything is written — so a document that would fail
/// to load on restart is rejected without corrupting the on-disk file. Reuses
/// the same atomic writer and `restart_needed` signal as the per-section HTTP
/// writers. Returns a plain error message on failure.
///
/// There is intentionally no unrestricted full-document writer: local
/// (trusted) config edits go through the per-section HTTP handlers
/// ([`apply_section_update`] / [`update_config_file`]), so every full-document
/// write path enforces [`REMOTE_PROTECTED_SECTIONS`].
pub async fn write_config_json_restricted(
    config_json: &str,
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
) -> Result<(), String> {
    let incoming: crate::config::RawConfig =
        serde_json::from_str(config_json).map_err(|e| format!("invalid config JSON: {e}"))?;

    let _lock = config_state.write_lock.lock().await;

    let current_toml =
        std::fs::read_to_string(&config_state.path).map_err(|e| format!("File read error: {e}"))?;
    let current: crate::config::RawConfig =
        toml::from_str(&current_toml).map_err(|e| format!("TOML parse error: {e}"))?;

    let incoming_value =
        serde_json::to_value(&incoming).map_err(|e| format!("JSON serialize error: {e}"))?;
    let current_value =
        serde_json::to_value(&current).map_err(|e| format!("JSON serialize error: {e}"))?;

    for section in REMOTE_PROTECTED_SECTIONS {
        if normalize_section(incoming_value.get(section))
            != normalize_section(current_value.get(section))
        {
            return Err(format!(
                "remote config may not modify the protected [{section}] section"
            ));
        }
    }

    write_config_json_locked(incoming, config_state, subsystem, ui_tx).await
}

/// Shared locked write body. Caller must hold `config_state.write_lock`.
///
/// Takes the already-parsed [`crate::config::RawConfig`] (the same value the
/// protected-section comparison ran against) so "what was compared is what is
/// written" holds structurally rather than by re-parsing the same string.
async fn write_config_json_locked(
    raw: crate::config::RawConfig,
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
) -> Result<(), String> {
    let new_toml =
        toml::to_string_pretty(&raw).map_err(|e| format!("TOML serialize error: {e}"))?;

    // Validate via the canonical loader so we never persist a config that would
    // fail to load on the next restart.
    crate::config::load_config_from_str(&new_toml, &config_state.path)
        .map_err(|e| format!("config validation failed: {e}"))?;

    write_atomic(&config_state.path, &new_toml).map_err(|e| format!("File write error: {e}"))?;

    mark_restart_needed_and_emit(subsystem, ui_tx).await;
    Ok(())
}

/// A missing section, `null`, and an all-null object are equivalent RawConfig
/// states.
///
/// Normalization is single-level by design: every protected raw section is a
/// flat struct of scalars/`Vec<String>` today. If a nested struct is ever
/// added to a protected section, an all-null nested object will not be
/// flattened — the failure mode is a false *reject* (fail-closed), and the
/// populated-section round-trip test in `p2p/remote_config.rs` will catch
/// serialization drift.
fn normalize_section(v: Option<&serde_json::Value>) -> serde_json::Value {
    match v {
        None | Some(serde_json::Value::Null) => serde_json::Value::Null,
        Some(serde_json::Value::Object(map)) if map.values().all(serde_json::Value::is_null) => {
            serde_json::Value::Null
        }
        Some(other) => other.clone(),
    }
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

fn validate_token_file(token_file: &str) -> Result<(), String> {
    if token_file.contains('\n') || token_file.contains('\r') {
        return Err("token_file must be a single-line path".to_owned());
    }
    Ok(())
}

fn validate_status_bind(bind: &str) -> Result<(), String> {
    bind.parse::<SocketAddrV4>()
        .map(|_| ())
        .map_err(|_| "bind must be a valid IPv4 address with port (e.g. 127.0.0.1:8080)".to_owned())
}

fn validate_server_url(url: &str) -> Result<(), String> {
    if url.starts_with("http://") || url.starts_with("https://") {
        Ok(())
    } else {
        Err("server_url must start with http:// or https://".to_owned())
    }
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
struct ServerDeviceStatusJson {
    configured: bool,
    endpoint_id: Option<String>,
    reachable: Option<bool>,
    approval_state: Option<String>,
    waiting_for_approval: bool,
    message: Option<String>,
}

impl ServerDeviceStatusJson {
    fn not_configured() -> Self {
        Self {
            configured: false,
            endpoint_id: None,
            reachable: None,
            approval_state: None,
            waiting_for_approval: false,
            message: None,
        }
    }
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
    server: ServerDeviceStatusJson,
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

#[derive(Debug, serde::Deserialize)]
struct ServerStatusBoardJson {
    #[serde(default)]
    devices: Vec<ServerStatusDeviceJson>,
}

#[derive(Debug, serde::Deserialize)]
struct ServerStatusDeviceJson {
    endpoint_id: String,
    approval_state: String,
}

async fn forwarder_server_status<J: JournalAccess + Send + 'static>(
    state: &AppState<J>,
) -> ServerDeviceStatusJson {
    let endpoint_id = state.subsystem.lock().await.p2p_endpoint_id.clone();
    let Some(endpoint_id) = endpoint_id else {
        return ServerDeviceStatusJson::not_configured();
    };
    let Some(config_state) = get_config_state(state) else {
        return ServerDeviceStatusJson::not_configured();
    };
    let server_url = {
        let _guard = config_state.write_lock.lock().await;
        match crate::config::load_config_from_path(&config_state.path) {
            Ok(config) => config.p2p.server_url,
            Err(error) => {
                return ServerDeviceStatusJson {
                    configured: true,
                    endpoint_id: Some(endpoint_id),
                    reachable: None,
                    approval_state: None,
                    waiting_for_approval: false,
                    message: Some(format!("Forwarder config unavailable: {error}")),
                };
            }
        }
    };
    let Some(server_url) = server_url else {
        return ServerDeviceStatusJson::not_configured();
    };

    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(1))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            return ServerDeviceStatusJson {
                configured: true,
                endpoint_id: Some(endpoint_id),
                reachable: Some(false),
                approval_state: None,
                waiting_for_approval: false,
                message: Some(format!("Server status client unavailable: {error}")),
            };
        }
    };
    let status_url = format!("{}/status", server_url.trim_end_matches('/'));
    let response = match client.get(status_url).send().await {
        Ok(response) => response,
        Err(error) => {
            return ServerDeviceStatusJson {
                configured: true,
                endpoint_id: Some(endpoint_id),
                reachable: Some(false),
                approval_state: None,
                waiting_for_approval: false,
                message: Some(format!("Server status unavailable: {error}")),
            };
        }
    };
    let board = match response.error_for_status() {
        Ok(response) => match response.json::<ServerStatusBoardJson>().await {
            Ok(board) => board,
            Err(error) => {
                return ServerDeviceStatusJson {
                    configured: true,
                    endpoint_id: Some(endpoint_id),
                    reachable: Some(false),
                    approval_state: None,
                    waiting_for_approval: false,
                    message: Some(format!("Server status response was invalid: {error}")),
                };
            }
        },
        Err(error) => {
            return ServerDeviceStatusJson {
                configured: true,
                endpoint_id: Some(endpoint_id),
                reachable: Some(false),
                approval_state: None,
                waiting_for_approval: false,
                message: Some(format!("Server status returned an error: {error}")),
            };
        }
    };

    match board
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == endpoint_id)
    {
        Some(device) => {
            let waiting_for_approval = device.approval_state == "pending";
            ServerDeviceStatusJson {
                configured: true,
                endpoint_id: Some(endpoint_id),
                reachable: Some(true),
                approval_state: Some(device.approval_state),
                waiting_for_approval,
                message: waiting_for_approval
                    .then(|| "Waiting for server admin approval".to_owned()),
            }
        }
        None => ServerDeviceStatusJson {
            configured: true,
            endpoint_id: Some(endpoint_id),
            reachable: Some(true),
            approval_state: None,
            waiting_for_approval: true,
            message: Some("Waiting for this forwarder to register with the server".to_owned()),
        },
    }
}

async fn status_json_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let server = forwarder_server_status(&state).await;
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
        p2p_connected: ss.p2p_connected(),
        restart_needed: ss.restart_needed(),
        ups_status: ss.ups_status().cloned(),
        server,
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
    let ss = state.subsystem.lock().await;
    #[cfg(any(feature = "eink", feature = "lcd"))]
    {
        let forwarder_name = state.display_name.lock().await.clone();
        let cpu_temp = *state.cpu_temp.lock().await;
        return axum::Json(
            serde_json::to_value(subsystem_to_display_state(&ss, forwarder_name, cpu_temp))
                .expect("display state serializes"),
        );
    }

    #[cfg(not(any(feature = "eink", feature = "lcd")))]
    {
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
                crate::ui_events::ForwarderUiEvent::UpsStatusChanged { .. } => "ups_status_changed",
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

/// Compute the target second boundary and pre-SET wait duration for clock sync.
///
/// Given the current wall time, one-way latency estimate, and the fixed sync delay,
/// returns `(target_boundary, pre_set_wait)` where:
/// - `target_boundary` is the `DateTime<Local>` whole-second that the rollover should align with
/// - `pre_set_wait` is how long to sleep before sending SET_DATE_TIME
pub fn compute_sync_timing(
    wall_now: chrono::DateTime<chrono::Local>,
    one_way: std::time::Duration,
    sync_delay_ms: u64,
) -> (chrono::DateTime<chrono::Local>, std::time::Duration) {
    use chrono::Timelike;

    let arrival_offset = chrono::Duration::from_std(one_way).unwrap_or_else(|_| {
        tracing::warn!(
            one_way_ms = one_way.as_millis(),
            "one-way latency exceeds chrono Duration range, falling back to zero"
        );
        chrono::Duration::zero()
    });
    let sync_delay = chrono::Duration::milliseconds(sync_delay_ms as i64);
    let wall_at_rollover_if_now = wall_now + arrival_offset + sync_delay;
    let rollover_frac = wall_at_rollover_if_now.nanosecond() as f64 / 1_000_000_000.0;

    let target = if rollover_frac >= 0.5 {
        wall_at_rollover_if_now + chrono::Duration::seconds(1)
    } else {
        wall_at_rollover_if_now
    };
    let target_boundary_initial = target
        .with_nanosecond(0)
        .expect("nanosecond 0 is always valid");

    let mut target_boundary = target_boundary_initial;
    let mut ideal_send = target_boundary - arrival_offset - sync_delay;
    if ideal_send < wall_now {
        target_boundary += chrono::Duration::seconds(1);
        ideal_send = target_boundary - arrival_offset - sync_delay;
    }
    let pre_set_wait = ideal_send
        .signed_duration_since(wall_now)
        .to_std()
        .unwrap_or(std::time::Duration::ZERO);

    (target_boundary, pre_set_wait)
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

        let mut ui_rx = server.ui_tx.subscribe();

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
            subsystem: server.subsystem.clone(),
            journal: Arc::new(Mutex::new(NoJournal)),
            version: Arc::new("0.2.0".to_owned()),
            config_state: None,
            restart_signal: None,
            logger: server.logger.clone(),
            ui_tx: server.ui_tx.clone(),
            status_event_tx: server.status_event_tx.clone(),
            control_clients: server.control_clients.clone(),
            download_trackers: server.download_trackers.clone(),
            reconnect_notifies: server.reconnect_notifies.clone(),
            #[cfg(any(feature = "eink", feature = "lcd"))]
            display_name: server.display_name.clone(),
            #[cfg(any(feature = "eink", feature = "lcd"))]
            cpu_temp: server.cpu_temp.clone(),
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

        let mut rx = server.ui_tx.subscribe();
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
            server.subsystem.lock().await.update_mode,
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
    async fn set_ready_with_display_sender_does_not_deadlock() {
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

        tokio::time::timeout(Duration::from_millis(100), server.set_ready())
            .await
            .expect("set_ready timed out");
        tokio::time::timeout(Duration::from_millis(100), display_rx.changed())
            .await
            .expect("display state publish timed out")
            .expect("display state sender dropped");
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
            let ss = server.subsystem.lock().await;
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

    #[test]
    fn sync_timing_rollover_frac_above_half_rounds_up() {
        use chrono::TimeZone;
        use chrono::Timelike;
        // wall_now=.200, one_way=50ms, sync=500ms → rollover_if_now=.750, frac=0.75 → rounds UP
        let wall_now = chrono::Local
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .unwrap()
            .with_nanosecond(200_000_000)
            .unwrap();
        let one_way = std::time::Duration::from_millis(50);
        let (target, wait) = super::compute_sync_timing(wall_now, one_way, 500);
        assert_eq!(target.second(), 1);
        assert_eq!(target.nanosecond(), 0);
        assert!(wait < std::time::Duration::from_secs(1));
    }

    #[test]
    fn sync_timing_rollover_frac_below_half_stays_same_second() {
        use chrono::TimeZone;
        use chrono::Timelike;
        // wall_now=.800, one_way=50ms, sync=500ms → rollover_if_now=1.350, frac=0.35 → truncates to 1.000
        // ideal_send=1.000-0.050-0.500=0.450 < 0.800 → BUMP → target=2.000
        let wall_now = chrono::Local
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .unwrap()
            .with_nanosecond(800_000_000)
            .unwrap();
        let one_way = std::time::Duration::from_millis(50);
        let (target, _wait) = super::compute_sync_timing(wall_now, one_way, 500);
        assert_eq!(target.second(), 2);
        assert_eq!(target.nanosecond(), 0);
    }

    #[test]
    fn sync_timing_ideal_send_past_bumps_target() {
        use chrono::TimeZone;
        use chrono::Timelike;
        // wall_now=.500, one_way=1ms, sync=500ms → rollover_if_now=1.001, frac=0.001 → target=1.000
        // ideal_send=1.000-0.001-0.500=0.499 < 0.500 → BUMP → target=2.000
        let wall_now = chrono::Local
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .unwrap()
            .with_nanosecond(500_000_000)
            .unwrap();
        let one_way = std::time::Duration::from_millis(1);
        let (target, wait) = super::compute_sync_timing(wall_now, one_way, 500);
        assert_eq!(target.second(), 2);
        assert_eq!(target.nanosecond(), 0);
        assert!(wait > std::time::Duration::from_millis(900));
        assert!(wait < std::time::Duration::from_millis(1100));
    }

    #[test]
    fn sync_timing_zero_latency() {
        use chrono::TimeZone;
        use chrono::Timelike;
        // wall_now=.300, one_way=0, sync=500ms → rollover_if_now=.800, frac=0.8 → rounds UP
        let wall_now = chrono::Local
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .unwrap()
            .with_nanosecond(300_000_000)
            .unwrap();
        let one_way = std::time::Duration::ZERO;
        let (target, _wait) = super::compute_sync_timing(wall_now, one_way, 500);
        assert_eq!(target.second(), 1);
        assert_eq!(target.nanosecond(), 0);
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
