//! Pure runtime status state for the forwarder.
//!
//! This module owns the in-memory status store and status-event fanout used by
//! the data plane, UI event stream, and P2P control status feed. It intentionally
//! has no HTTP/axum dependencies.

use rt_updater::UpdateStatus;
use rt_updater::workflow::WorkflowState;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, Notify, broadcast};

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

/// Cached snapshot of this forwarder's registration status on the central
/// server, refreshed by the background poll task (`server_status_task`).
///
/// The `/api/v1/status` handler serves this snapshot directly and never
/// performs outbound I/O; `checked_unix_ms` is the staleness signal (None
/// until the first poll completes).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerDeviceStatus {
    pub configured: bool,
    pub endpoint_id: Option<String>,
    pub reachable: Option<bool>,
    pub approval_state: Option<String>,
    pub waiting_for_approval: bool,
    pub message: Option<String>,
    /// Always `true`: signals to clients that this snapshot is served from
    /// the poll cache rather than a live server round-trip.
    pub cached: bool,
    /// Unix ms timestamp of the poll that produced this snapshot.
    pub checked_unix_ms: Option<i64>,
}

impl ServerDeviceStatus {
    /// Shape served when no server is configured — and before the first poll
    /// completes (with `checked_unix_ms: None`).
    pub fn not_configured() -> Self {
        Self {
            configured: false,
            endpoint_id: None,
            reachable: None,
            approval_state: None,
            waiting_for_approval: false,
            message: None,
            cached: true,
            checked_unix_ms: None,
        }
    }

    /// Compare the UI-meaningful fields of two snapshots, ignoring the
    /// bookkeeping fields (`cached` and `checked_unix_ms`) that change on
    /// every poll.
    pub(crate) fn ui_meaningful_eq(&self, other: &Self) -> bool {
        // Destructure so adding a field to ServerDeviceStatus is a compile
        // error here instead of a silent gating bug.
        let Self {
            configured,
            endpoint_id,
            reachable,
            approval_state,
            waiting_for_approval,
            message,
            cached: _,
            checked_unix_ms: _,
        } = self;
        *configured == other.configured
            && *endpoint_id == other.endpoint_id
            && *reachable == other.reachable
            && *approval_state == other.approval_state
            && *waiting_for_approval == other.waiting_for_approval
            && *message == other.message
    }
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
    pub(crate) ready: bool,
    pub(crate) reason: Option<String>,
    /// P2P session state is tracked for the status page but does NOT affect readiness.
    pub(crate) p2p_connected: bool,
    pub(crate) p2p_endpoint_id: Option<String>,
    pub(crate) forwarder_id: String,
    pub(crate) local_ip: Option<String>,
    pub(crate) readers: HashMap<String, ReaderStatus>,
    pub(crate) update_status: UpdateStatus,
    pub(crate) staged_update_path: Option<std::path::PathBuf>,
    pub update_mode: rt_updater::UpdateMode,
    /// Set to `true` when config is saved and the forwarder needs a restart to apply changes.
    pub(crate) restart_needed: bool,
    /// UPS status snapshot (None if UPS monitoring is not configured).
    pub(crate) ups_status: Option<UpsStatusState>,
    /// Cached server reachability snapshot (None until the first poll).
    pub(crate) server_status: Option<ServerDeviceStatus>,
    /// Readers whose read counters changed since the last coalesced P2P
    /// status broadcast (see [`spawn_read_count_broadcaster`]).
    pub(crate) read_counts_dirty: HashSet<String>,
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
            server_status: None,
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
            server_status: None,
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

    /// Set the cached server reachability snapshot.
    ///
    /// Returns `true` when the new snapshot differs from the previous one in
    /// UI-meaningful fields (see [`ServerDeviceStatus::ui_meaningful_eq`]).
    /// Before the first poll the status endpoint serves the not-configured
    /// baseline, so the first snapshot is compared against that.
    pub fn set_server_status(&mut self, status: ServerDeviceStatus) -> bool {
        let changed = match &self.server_status {
            Some(prev) => !prev.ui_meaningful_eq(&status),
            None => !ServerDeviceStatus::not_configured().ui_meaningful_eq(&status),
        };
        self.server_status = Some(status);
        changed
    }

    /// Return the cached server reachability snapshot, if any poll completed.
    pub fn server_status(&self) -> Option<&ServerDeviceStatus> {
        self.server_status.as_ref()
    }
}

// ---------------------------------------------------------------------------
// StatusStore handle
// ---------------------------------------------------------------------------

/// Pure shared status state used by the data plane and HTTP adapter.
#[derive(Clone)]
pub struct StatusStore {
    subsystem: Arc<Mutex<SubsystemStatus>>,
    ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
    status_event_tx: broadcast::Sender<ForwarderStatusEvent>,
    logger: Arc<rt_ui_log::UiLogger<crate::ui_events::ForwarderUiEvent>>,
    #[cfg(any(feature = "eink", feature = "lcd"))]
    display_tx:
        Arc<std::sync::Mutex<Option<tokio::sync::watch::Sender<rt_screen::state::DisplayState>>>>,
    #[cfg(any(feature = "eink", feature = "lcd"))]
    display_name: Arc<Mutex<Option<String>>>,
    #[cfg(any(feature = "eink", feature = "lcd"))]
    cpu_temp: Arc<Mutex<Option<f32>>>,
    control_clients:
        Arc<std::sync::RwLock<HashMap<String, Arc<crate::reader_control::ControlClient>>>>,
    download_trackers: Arc<
        std::sync::RwLock<
            HashMap<String, Arc<tokio::sync::Mutex<crate::reader_control::DownloadTracker>>>,
        >,
    >,
    reconnect_notifies: Arc<std::sync::RwLock<HashMap<String, Arc<Notify>>>>,
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
pub(crate) async fn broadcast_dirty_read_counts(
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

/// Workflow state adapter over the forwarder's shared status store.
#[derive(Clone)]
pub(crate) struct ForwarderWorkflowAdapter {
    store: StatusStore,
}

impl WorkflowState for ForwarderWorkflowAdapter {
    fn current_status<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = UpdateStatus> + Send + 'a>> {
        Box::pin(async move { self.store.update_status().await })
    }

    fn set_status<'a>(
        &'a self,
        status: UpdateStatus,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.store.set_update_status_without_emit(status).await;
        })
    }

    fn set_downloaded<'a>(
        &'a self,
        status: UpdateStatus,
        path: std::path::PathBuf,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.store.set_downloaded_update(status, path).await;
        })
    }

    fn emit_status_changed<'a>(
        &'a self,
        status: UpdateStatus,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.store.emit_update_status_changed(status);
        })
    }
}

impl StatusStore {
    /// Create a status store with fresh UI and P2P status broadcast channels.
    pub fn new(subsystem: SubsystemStatus) -> Self {
        let (ui_tx, _) = tokio::sync::broadcast::channel(256);
        let (status_event_tx, _) = broadcast::channel(256);
        let logger = Arc::new(rt_ui_log::UiLogger::with_buffer(
            ui_tx.clone(),
            |entry| crate::ui_events::ForwarderUiEvent::LogEntry { entry },
            500,
        ));
        let subsystem = Arc::new(Mutex::new(subsystem));
        let store = Self {
            subsystem: subsystem.clone(),
            ui_tx,
            status_event_tx: status_event_tx.clone(),
            logger,
            #[cfg(any(feature = "eink", feature = "lcd"))]
            display_tx: Arc::new(std::sync::Mutex::new(None)),
            #[cfg(any(feature = "eink", feature = "lcd"))]
            display_name: Arc::new(Mutex::new(None)),
            #[cfg(any(feature = "eink", feature = "lcd"))]
            cpu_temp: Arc::new(Mutex::new(None)),
            control_clients: Arc::new(std::sync::RwLock::new(HashMap::new())),
            download_trackers: Arc::new(std::sync::RwLock::new(HashMap::new())),
            reconnect_notifies: Arc::new(std::sync::RwLock::new(HashMap::new())),
        };
        spawn_read_count_broadcaster(subsystem, status_event_tx);
        store
    }

    /// Return a clone of the P2P status event sender for HTTP adapter state.
    pub(crate) fn status_event_sender(&self) -> broadcast::Sender<ForwarderStatusEvent> {
        self.status_event_tx.clone()
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
        self.publish_display_state().await;
    }

    /// Set the forwarder ID (call once at startup).
    pub async fn set_forwarder_id(&self, id: &str) {
        self.subsystem.lock().await.forwarder_id = id.to_owned();
    }

    /// Set the detected local IP (call once at startup).
    pub async fn set_local_ip(&self, ip: Option<String>) {
        self.subsystem.lock().await.local_ip = ip;
        self.publish_display_state().await;
    }

    /// Set the update mode (controls check-only vs check-and-download behavior).
    pub async fn set_update_mode(&self, mode: rt_updater::UpdateMode) {
        self.subsystem.lock().await.update_mode = mode;
    }

    /// Return the configured update mode.
    pub async fn update_mode(&self) -> rt_updater::UpdateMode {
        self.subsystem.lock().await.update_mode
    }

    /// Return the current rt-updater status.
    pub async fn update_status(&self) -> UpdateStatus {
        self.subsystem.lock().await.update_status.clone()
    }

    /// Return the staged update artifact path, if one exists.
    pub async fn staged_update_path(&self) -> Option<std::path::PathBuf> {
        self.subsystem.lock().await.staged_update_path.clone()
    }

    /// Update the current rt-updater status (shown on `/update/status`).
    pub async fn set_update_status(&self, status: UpdateStatus) {
        self.set_update_status_without_emit(status.clone()).await;
        self.emit_update_status_changed(status);
    }

    async fn set_update_status_without_emit(&self, status: UpdateStatus) {
        self.subsystem.lock().await.update_status = status;
    }

    async fn set_downloaded_update(&self, status: UpdateStatus, path: std::path::PathBuf) {
        let mut ss = self.subsystem.lock().await;
        ss.update_status = status;
        ss.staged_update_path = Some(path);
    }

    fn emit_update_status_changed(&self, status: UpdateStatus) {
        let _ = self
            .ui_tx
            .send(crate::ui_events::ForwarderUiEvent::UpdateStatusChanged { status });
    }

    pub(crate) fn workflow_state(&self) -> ForwarderWorkflowAdapter {
        ForwarderWorkflowAdapter {
            store: self.clone(),
        }
    }

    /// Record the filesystem path of a downloaded update artifact ready to apply.
    pub async fn set_staged_update_path(&self, path: std::path::PathBuf) {
        self.subsystem.lock().await.staged_update_path = Some(path);
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub fn set_display_sender(
        &self,
        tx: tokio::sync::watch::Sender<rt_screen::state::DisplayState>,
    ) {
        *self.display_tx.lock().unwrap_or_else(|e| e.into_inner()) = Some(tx);
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

    #[cfg(any(feature = "eink", feature = "lcd"))]
    pub async fn display_state(&self) -> rt_screen::state::DisplayState {
        let ss = self.subsystem.lock().await;
        let forwarder_name = self.display_name.lock().await.clone();
        let cpu_temp = *self.cpu_temp.lock().await;
        subsystem_to_display_state(&ss, forwarder_name, cpu_temp)
    }

    #[cfg(any(feature = "eink", feature = "lcd"))]
    async fn publish_display_state(&self) {
        let tx = self
            .display_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if let Some(tx) = tx {
            tx.send_replace(self.display_state().await);
        }
    }

    #[cfg(not(any(feature = "eink", feature = "lcd")))]
    async fn publish_display_state(&self) {}

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
        self.publish_display_state().await;
    }

    /// Store the latest server reachability snapshot from the poll task,
    /// notifying the UI only when UI-meaningful fields changed.
    pub async fn set_server_status(&self, status: ServerDeviceStatus) {
        // Emit while holding the subsystem lock so state change and UI event
        // stay ordered, matching set_ready/set_p2p_connected/set_ups_status.
        let mut ss = self.subsystem.lock().await;
        let changed = ss.set_server_status(status.clone());
        if changed {
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ServerStatusChanged { server: status });
        }
        drop(ss);
    }

    /// Return the cached server reachability snapshot, if any poll completed.
    pub async fn server_status(&self) -> Option<ServerDeviceStatus> {
        self.subsystem.lock().await.server_status().cloned()
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
        self.publish_display_state().await;
    }
}

pub(crate) async fn mark_restart_needed_and_emit(
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::broadcast::error::TryRecvError;

    fn sample_server_status(checked_unix_ms: Option<i64>) -> ServerDeviceStatus {
        ServerDeviceStatus {
            configured: true,
            endpoint_id: Some("node-1".to_owned()),
            reachable: Some(true),
            approval_state: Some("active".to_owned()),
            waiting_for_approval: false,
            message: None,
            cached: true,
            checked_unix_ms,
        }
    }

    #[tokio::test]
    async fn set_server_status_emits_ui_event_only_on_meaningful_change() {
        let store = StatusStore::new(SubsystemStatus::ready());
        let mut rx = store.ui_sender().subscribe();

        // First poll differs from the not-configured baseline: one event.
        store
            .set_server_status(sample_server_status(Some(1_000)))
            .await;
        match rx.try_recv() {
            Ok(crate::ui_events::ForwarderUiEvent::ServerStatusChanged { server }) => {
                assert_eq!(server.endpoint_id.as_deref(), Some("node-1"));
            }
            other => panic!("expected ServerStatusChanged, got {other:?}"),
        }

        // Identical snapshot except checked_unix_ms (the common 30s poll case):
        // no event.
        store
            .set_server_status(sample_server_status(Some(31_000)))
            .await;
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));

        // UI-meaningful change (reachable flips): exactly one event.
        let mut changed = sample_server_status(Some(61_000));
        changed.reachable = Some(false);
        changed.message = Some("server unreachable".to_owned());
        store.set_server_status(changed).await;
        assert!(matches!(
            rx.try_recv(),
            Ok(crate::ui_events::ForwarderUiEvent::ServerStatusChanged { .. })
        ));
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }

    #[tokio::test]
    async fn set_server_status_no_event_when_first_poll_matches_baseline() {
        let store = StatusStore::new(SubsystemStatus::ready());
        let mut rx = store.ui_sender().subscribe();

        // Before the first poll the status endpoint serves the not-configured
        // baseline, so a first poll with the same UI-meaningful fields is not
        // a change.
        let status = ServerDeviceStatus {
            checked_unix_ms: Some(1_000),
            ..ServerDeviceStatus::not_configured()
        };
        store.set_server_status(status).await;
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Empty)));
    }
}
