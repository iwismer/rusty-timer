//! Receiver control API — business logic for the receiver.
//!
//! All handler functions are plain async functions that take `&AppState`
//! and return `Result<T, ReceiverError>`.  The Tauri app wraps these as
//! IPC commands.

use crate::db::{DEFAULT_UPDATE_MODE, Db, StreamSubscription};
use crate::error::ReceiverError;
use crate::ui_events::ReceiverUiEvent;
use rt_domain::ReceiverMode;
use rt_p2p_protocol::{
    ConfigGetResponse, ConfigSetResponse, DownloadProgress, ReaderControlResponse, ReaderInfo,
    ReaderStatus, RestartResponse, UpsStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

pub type ChipLookup = HashMap<String, HashMap<String, (String, String)>>;

/// One stream a discovered forwarder exposes, learned from the server
/// `GET /forwarders` discovery feed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredStream {
    pub stream_id: String,
    pub epoch: i64,
    pub next_seq: i64,
}

/// An approved forwarder discovered from the server (or seeded from an
/// explicit local forwarder config). `direct_addrs` are the addresses the
/// receiver dials; `streams` is the forwarder's advertised stream catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredForwarder {
    pub display_name: Option<String>,
    pub direct_addrs: Vec<SocketAddr>,
    pub streams: Vec<DiscoveredStream>,
}

/// Shared map of discovered forwarders keyed by their endpoint id (string node
/// id). Populated by the discovery task and/or seeded from explicit config.
pub type DiscoveredForwarders = HashMap<String, DiscoveredForwarder>;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::{Mutex, RwLock, broadcast, mpsc, mpsc::error::TrySendError, oneshot, watch};
use tracing::warn;

/// How long a remote-config command waits for the forwarder's response before
/// failing, so a missing/late reply can never hang the caller. The response is
/// routed back to the awaiting command by the per-forwarder control loop.
pub(crate) const FORWARDER_CONFIG_TIMEOUT: Duration = Duration::from_secs(10);

/// A remote-config request bridged from a control-API command (`&AppState`) to
/// the live [`ForwarderConnection`](crate::p2p_forwarder) control loop that
/// owns the QUIC control session. Each variant carries a `oneshot` responder
/// the control loop completes once the forwarder replies (routed by
/// `request_id`). Only registered for forwarders whose live session negotiated
/// `CAP_REMOTE_CONFIG`.
pub(crate) enum ConfigCommand {
    Get {
        resp: oneshot::Sender<ConfigGetResponse>,
    },
    Set {
        config_json: String,
        resp: oneshot::Sender<ConfigSetResponse>,
    },
    Restart {
        resp: oneshot::Sender<RestartResponse>,
    },
}

/// A reader-control request bridged from control API commands to the live
/// P2P control loop for a forwarder that negotiated `CAP_READER_CONTROL`.
pub(crate) enum ReaderCommand {
    Request {
        stream_id: String,
        action: rt_domain::ReaderControlAction,
        resp: oneshot::Sender<ReaderControlResponse>,
    },
}

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ForwarderConnState {
    Subscribed,
    Connected,
    Unavailable,
    Disconnected,
}

/// A point-in-time view of one forwarder's connection state.
///
/// Contract for consumers (notably the UI): while `pending == true` the
/// forwarder is still within the initial 5s connect grace window
/// ([`FORWARDER_PENDING_GRACE`]) after a dial attempt began, and consumers MUST
/// present it as a transient "connecting" status — NOT as the underlying
/// `state`, which during the grace window is reported as
/// [`ForwarderConnState::Unavailable`] even though no real failure has been
/// confirmed yet. Only once `pending == false` does `state` reflect the
/// settled, user-facing condition. (Phase 2 UI maps `pending` → an amber
/// "Connecting…" badge.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ForwarderStateSnapshot {
    pub state: ForwarderConnState,
    pub pending: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectAttempt {
    pub version: u64,
    pub endpoint_id: Option<String>,
    pub restart: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ForwarderRuntimeStatus {
    pub control_up: bool,
    pub data_sessions: usize,
    pub pending_started_at: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConnectionsFingerprint {
    aggregate_state: ConnectionState,
    forwarders: Vec<ForwarderConnectionsFingerprint>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForwarderConnectionsFingerprint {
    endpoint_id: String,
    state: ForwarderConnState,
    pending: bool,
    intent: bool,
    discovered: bool,
    display_name: Option<String>,
    remote_config_available: bool,
    reader_control_available: bool,
    subscribed_count: usize,
    available_count: usize,
    readers: Vec<ReaderLiveStatus>,
    ups: Option<UpsStatusPayload>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ForwarderLiveStatus {
    readers: HashMap<String, ReaderLiveStatus>,
    ups: Option<UpsStatusPayload>,
}

const FORWARDER_PENDING_GRACE: Duration = Duration::from_secs(5);

fn derive_forwarder_state(runtime: ForwarderRuntimeStatus, intent: bool) -> ForwarderStateSnapshot {
    if runtime.data_sessions > 0 {
        return ForwarderStateSnapshot {
            state: ForwarderConnState::Subscribed,
            pending: false,
        };
    }
    if runtime.control_up {
        return ForwarderStateSnapshot {
            state: ForwarderConnState::Connected,
            pending: false,
        };
    }
    if intent {
        let pending = runtime
            .pending_started_at
            .is_some_and(|started| started.elapsed() < FORWARDER_PENDING_GRACE);
        return ForwarderStateSnapshot {
            state: ForwarderConnState::Unavailable,
            pending,
        };
    }
    ForwarderStateSnapshot {
        state: ForwarderConnState::Disconnected,
        pending: false,
    }
}

fn decode_stream_id(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes)
        .unwrap_or_else(|error| String::from_utf8_lossy(&error.into_bytes()).into_owned())
}

fn optional_non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn parse_reader_info_json(
    reader_info_json: Option<&str>,
    endpoint_id: &str,
    stream_id: &str,
) -> Option<rt_domain::ReaderInfo> {
    let json = reader_info_json.filter(|json| !json.is_empty())?;
    match serde_json::from_str(json) {
        Ok(reader_info) => Some(reader_info),
        Err(error) => {
            warn!(%endpoint_id, %stream_id, %error, "forwarder sent invalid reader_info_json");
            None
        }
    }
}

fn download_state_from_str(state: &str) -> rt_domain::DownloadState {
    match state {
        "downloading" => rt_domain::DownloadState::Downloading,
        "complete" => rt_domain::DownloadState::Complete,
        "error" => rt_domain::DownloadState::Error,
        _ => rt_domain::DownloadState::Idle,
    }
}

fn subscription_local_ports(
    subscriptions: &[StreamSubscription],
) -> HashMap<(String, String), Option<u16>> {
    subscriptions
        .iter()
        .map(|subscription| {
            let display_reader_ip = subscription.reader_ip.clone().or_else(|| {
                crate::ports::reader_addr_if_port_mappable(&subscription.stream_id)
                    .map(std::borrow::ToOwned::to_owned)
            });
            let local_port = subscription.local_port_override.or_else(|| {
                display_reader_ip
                    .as_deref()
                    .and_then(crate::ports::default_port)
            });
            (
                (
                    subscription.forwarder_endpoint_id.clone(),
                    subscription.stream_id.clone(),
                ),
                local_port,
            )
        })
        .collect()
}

fn sorted_reader_statuses(
    live_status: &ForwarderLiveStatus,
    local_ports: &HashMap<(String, String), Option<u16>>,
    endpoint_id: &str,
) -> Vec<ReaderLiveStatus> {
    let mut readers = live_status.readers.values().cloned().collect::<Vec<_>>();
    for reader in &mut readers {
        reader.local_port = local_ports
            .get(&(endpoint_id.to_owned(), reader.stream_id.clone()))
            .copied()
            .flatten();
    }
    readers.sort_by(|a, b| a.stream_id.cmp(&b.stream_id));
    readers
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    None,
    Disconnect,
    Terminate,
}

pub struct AppState {
    pub db: Arc<Mutex<Db>>,
    pub connection_state: watch::Sender<ConnectionState>,
    // Keepalive receiver so that `connection_state.send()` never fails due
    // to "no receivers" even when no external subscriber is active.
    _conn_state_keepalive: watch::Receiver<ConnectionState>,
    pub logger: Arc<rt_ui_log::UiLogger<ReceiverUiEvent>>,
    pub shutdown_tx: watch::Sender<ShutdownSignal>,
    pub ui_tx: broadcast::Sender<ReceiverUiEvent>,
    pub stream_counts: crate::cache::StreamCounts,
    pub stream_metrics_cache:
        Arc<RwLock<HashMap<(String, String), crate::ui_events::StreamMetricsPayload>>>,
    pub receiver_id: Arc<RwLock<String>>,
    pub db_integrity_ok: bool,
    pub http_client: reqwest::Client,
    pub chip_lookup: Arc<tokio::sync::RwLock<ChipLookup>>,
    /// Approved forwarders discovered from the server (or seeded from an
    /// explicit local forwarder config), keyed by endpoint id. Drives both the
    /// available-but-unsubscribed entries in the streams response and the
    /// per-subscription dial address resolution in the P2P runtime.
    pub discovered_forwarders: Arc<tokio::sync::RwLock<DiscoveredForwarders>>,
    pub p2p_endpoint_id: Arc<RwLock<Option<String>>>,
    forwarder_runtime: Arc<StdMutex<HashMap<String, ForwarderRuntimeStatus>>>,
    forwarder_live_status: Arc<StdMutex<HashMap<String, ForwarderLiveStatus>>>,
    /// Per-forwarder remote-config request channels, keyed by endpoint id. An
    /// entry exists only while that forwarder has a live control session whose
    /// negotiated `HelloOk` advertised `CAP_REMOTE_CONFIG`; the
    /// [`ForwarderConnection`](crate::p2p_forwarder) registers its sender on
    /// connect and deregisters on disconnect/stop. Presence therefore doubles
    /// as the `remote_config_available` signal, so a command to a down or
    /// incapable forwarder fails fast.
    forwarder_config_tx: StdMutex<HashMap<String, mpsc::Sender<ConfigCommand>>>,
    forwarder_reader_control_tx: StdMutex<HashMap<String, mpsc::Sender<ReaderCommand>>>,
    last_connections_fingerprint: StdMutex<Option<ConnectionsFingerprint>>,
    connect_attempt: AtomicU64,
    connect_attempt_version: watch::Sender<ConnectAttempt>,
    /// Keepalive receiver to prevent the connect-attempt watch channel from being dropped
    /// when no runtime subscriber is active.
    _connect_attempt_keepalive: watch::Receiver<ConnectAttempt>,
    retry_streak: AtomicU64,
    /// Monotonic counter incremented when DBF config changes; subscribers
    /// (runtime.rs) use this to restart the DBF writer. Use
    /// `notify_dbf_config_changed()` and `dbf_config_rx()` to interact.
    dbf_config_version: watch::Sender<u64>,
    /// Keepalive receiver to prevent the watch channel from being dropped
    /// when no external subscribers exist.
    _dbf_config_keepalive: watch::Receiver<u64>,
    /// Monotonic counter incremented when the server URL+token changes (e.g. a
    /// profile save); the P2P reconcile loop uses this to rebind server-bound
    /// tasks. Use `notify_server_config_changed()` and `server_config_rx()`.
    server_config_version: watch::Sender<u64>,
    /// Keepalive receiver so the watch channel is not dropped when the P2P
    /// runtime (the only subscriber) is not yet started.
    _server_config_keepalive: watch::Receiver<u64>,
    /// The raw server URL+token override, set once at startup: env vars for the
    /// desktop app, CLI flags for headless. Source of truth for "is an override
    /// active" and for resolving the effective server in control handlers, so
    /// both desktop and headless report/persist the same server the P2P runtime
    /// targets. `(None, None)` when there is no override source.
    server_override: tokio::sync::RwLock<(Option<String>, Option<String>)>,
}

impl AppState {
    pub fn new(db: Db, receiver_id: String) -> (Arc<Self>, watch::Receiver<ShutdownSignal>) {
        Self::with_integrity(db, receiver_id, true)
    }

    pub fn with_integrity(
        db: Db,
        receiver_id: String,
        db_integrity_ok: bool,
    ) -> (Arc<Self>, watch::Receiver<ShutdownSignal>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(ShutdownSignal::None);
        let (ui_tx, _) = broadcast::channel(256);
        let (conn_tx, conn_keepalive_rx) = watch::channel(ConnectionState::Disconnected);
        let (connect_attempt_version, _connect_attempt_keepalive) =
            watch::channel(ConnectAttempt {
                version: 0,
                endpoint_id: None,
                restart: false,
            });
        let (dbf_config_version, _dbf_config_keepalive) = watch::channel(0u64);
        let (server_config_version, _server_config_keepalive) = watch::channel(0u64);
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(3))
            .build()
            .expect("failed to build HTTP client");
        let state = Arc::new(Self {
            db: Arc::new(Mutex::new(db)),
            connection_state: conn_tx,
            _conn_state_keepalive: conn_keepalive_rx,
            logger: Arc::new(rt_ui_log::UiLogger::with_buffer(
                ui_tx.clone(),
                |entry| ReceiverUiEvent::LogEntry { entry },
                500,
            )),
            shutdown_tx,
            ui_tx,
            stream_counts: crate::cache::StreamCounts::new(),
            stream_metrics_cache: Arc::new(RwLock::new(HashMap::new())),
            receiver_id: Arc::new(RwLock::new(receiver_id)),
            db_integrity_ok,
            http_client,
            chip_lookup: Arc::new(tokio::sync::RwLock::new(ChipLookup::new())),
            discovered_forwarders: Arc::new(tokio::sync::RwLock::new(DiscoveredForwarders::new())),
            p2p_endpoint_id: Arc::new(RwLock::new(None)),
            forwarder_runtime: Arc::new(StdMutex::new(HashMap::new())),
            forwarder_live_status: Arc::new(StdMutex::new(HashMap::new())),
            forwarder_config_tx: StdMutex::new(HashMap::new()),
            forwarder_reader_control_tx: StdMutex::new(HashMap::new()),
            last_connections_fingerprint: StdMutex::new(None),
            connect_attempt: AtomicU64::new(0),
            connect_attempt_version,
            _connect_attempt_keepalive,
            retry_streak: AtomicU64::new(0),
            dbf_config_version,
            _dbf_config_keepalive,
            server_config_version,
            _server_config_keepalive,
            server_override: tokio::sync::RwLock::new((None, None)),
        });
        (state, shutdown_rx)
    }

    /// Record the server URL+token override (env for desktop, CLI for headless).
    /// Called once at startup before control handlers serve.
    pub async fn set_server_override(&self, override_: (Option<String>, Option<String>)) {
        *self.server_override.write().await = override_;
    }

    /// The server URL+token override captured at startup.
    pub async fn server_override(&self) -> (Option<String>, Option<String>) {
        self.server_override.read().await.clone()
    }

    /// Subscribe to connection state changes.
    pub fn conn_rx(&self) -> watch::Receiver<ConnectionState> {
        self.connection_state.subscribe()
    }

    pub fn notify_dbf_config_changed(&self) {
        self.dbf_config_version.send_modify(|v| *v += 1);
    }

    pub fn dbf_config_rx(&self) -> watch::Receiver<u64> {
        self.dbf_config_version.subscribe()
    }

    /// Signal that the server URL+token configuration changed so the P2P
    /// reconcile loop rebinds its server-bound tasks.
    pub fn notify_server_config_changed(&self) {
        self.server_config_version.send_modify(|v| *v += 1);
    }

    pub fn server_config_rx(&self) -> watch::Receiver<u64> {
        self.server_config_version.subscribe()
    }

    pub fn connect_attempt_rx(&self) -> watch::Receiver<ConnectAttempt> {
        self.connect_attempt_version.subscribe()
    }

    fn bump_connect_attempt(&self, endpoint_id: Option<String>, restart: bool) -> u64 {
        let next = self.connect_attempt.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.connect_attempt_version.send(ConnectAttempt {
            version: next,
            endpoint_id,
            restart,
        });
        next
    }

    pub async fn cache_stream_metrics(&self, payload: &crate::ui_events::StreamMetricsPayload) {
        let key = (payload.forwarder_id.clone(), payload.reader_ip.clone());
        self.stream_metrics_cache
            .write()
            .await
            .insert(key, payload.clone());
    }

    pub async fn clear_stream_metrics_cache(&self) {
        self.stream_metrics_cache.write().await.clear();
    }

    pub async fn set_p2p_endpoint_id(&self, endpoint_id: String) {
        *self.p2p_endpoint_id.write().await = Some(endpoint_id);
    }

    pub async fn get_stream_metrics_snapshot(&self) -> Vec<crate::ui_events::StreamMetricsPayload> {
        self.stream_metrics_cache
            .read()
            .await
            .values()
            .cloned()
            .collect()
    }

    pub fn request_disconnect_shutdown(&self) {
        let _ = self.shutdown_tx.send(ShutdownSignal::Disconnect);
    }

    pub fn request_process_shutdown(&self) {
        let _ = self.shutdown_tx.send(ShutdownSignal::Terminate);
    }

    pub fn current_connect_attempt(&self) -> u64 {
        self.connect_attempt.load(Ordering::SeqCst)
    }

    pub fn current_retry_streak(&self) -> u64 {
        self.retry_streak.load(Ordering::SeqCst)
    }

    pub fn reset_retry_streak(&self) {
        self.retry_streak.store(0, Ordering::SeqCst);
    }

    pub async fn request_connect(&self) {
        self.reset_retry_streak();
        self.bump_connect_attempt(None, true);
        self.set_connection_state(ConnectionState::Connecting).await;
    }

    pub async fn request_forwarder_reconnect(&self, endpoint_id: String) {
        self.reset_retry_streak();
        self.bump_connect_attempt(Some(endpoint_id), true);
        self.set_connection_state(ConnectionState::Connecting).await;
    }

    fn wake_reconcile(&self) {
        self.bump_connect_attempt(None, false);
    }

    pub(crate) fn update_forwarder_runtime_sync(
        &self,
        endpoint_id: &str,
        update: impl FnOnce(&mut ForwarderRuntimeStatus),
    ) {
        let mut statuses = self.forwarder_runtime.lock().unwrap();
        let status = statuses.entry(endpoint_id.to_owned()).or_default();
        update(status);
    }

    pub(crate) async fn mark_forwarder_runtime(
        &self,
        endpoint_id: &str,
        update: impl FnOnce(&mut ForwarderRuntimeStatus),
    ) {
        self.update_forwarder_runtime_sync(endpoint_id, update);
        self.recompute_aggregate_connection_state().await;
    }

    /// Start the per-forwarder pending grace clock when a dial attempt begins,
    /// but only if it is not already running. This is idempotent: repeated calls
    /// while still dialing (e.g. across reconnect retries) do not reset the
    /// clock, so the grace window measures from the *first* dial attempt and can
    /// actually elapse to [`ForwarderConnState::Unavailable`]. The clock is
    /// cleared on control connect and re-armed on control disconnect.
    pub async fn mark_forwarder_dial_started(&self, endpoint_id: &str) {
        self.mark_forwarder_runtime(endpoint_id, |status| {
            if status.pending_started_at.is_none() {
                status.pending_started_at = Some(Instant::now());
            }
        })
        .await;
    }

    pub async fn forwarder_state(&self, endpoint_id: &str) -> ForwarderStateSnapshot {
        let runtime = self
            .forwarder_runtime
            .lock()
            .unwrap()
            .get(endpoint_id)
            .copied()
            .unwrap_or_default();
        let intent = self
            .db
            .lock()
            .await
            .forwarder_should_connect(endpoint_id)
            .unwrap_or(true);
        derive_forwarder_state(runtime, intent)
    }

    #[cfg(test)]
    pub(crate) async fn record_forwarder_reader_status(
        &self,
        endpoint_id: &str,
        status: ReaderStatus,
    ) {
        self.store_forwarder_reader_status_sync(endpoint_id, status);
        self.recompute_aggregate_connection_state().await;
    }

    /// Quick, lock-only store of a reader's live status with NO aggregate
    /// recompute. Used on the control read path so a status frame never blocks
    /// the reader (and any queued heartbeat `Ping`) on the DB/discovered locks
    /// the recompute takes. Callers must trigger the recompute off-path.
    pub(crate) fn store_forwarder_reader_status_sync(
        &self,
        endpoint_id: &str,
        status: ReaderStatus,
    ) {
        let stream_id = decode_stream_id(status.stream_id);
        let reader = ReaderLiveStatus {
            stream_id: stream_id.clone(),
            connected: status.connected,
            state: status.state,
            last_read_unix_ms: (status.last_read_unix_ms != 0).then_some(status.last_read_unix_ms),
            hardware_reader_id: None,
            firmware_version: None,
            model: None,
            reader_info: None,
            download_progress: None,
            local_port: None,
        };
        let mut live_statuses = self.forwarder_live_status.lock().unwrap();
        let live_status = live_statuses.entry(endpoint_id.to_owned()).or_default();
        live_status
            .readers
            .entry(stream_id)
            .and_modify(|existing| {
                let hardware_reader_id = existing.hardware_reader_id.clone();
                let firmware_version = existing.firmware_version.clone();
                let model = existing.model.clone();
                let reader_info = existing.reader_info.clone();
                let download_progress = existing.download_progress.clone();
                let local_port = existing.local_port;
                *existing = ReaderLiveStatus {
                    hardware_reader_id,
                    firmware_version,
                    model,
                    reader_info,
                    download_progress,
                    local_port,
                    ..reader.clone()
                };
            })
            .or_insert(reader);
    }

    #[cfg(test)]
    pub(crate) async fn record_forwarder_reader_info(&self, endpoint_id: &str, info: ReaderInfo) {
        self.store_forwarder_reader_info_sync(endpoint_id, info);
        self.recompute_aggregate_connection_state().await;
    }

    /// Quick, lock-only store of a reader's static info with NO aggregate
    /// recompute (see [`Self::store_forwarder_reader_status_sync`]).
    pub(crate) fn store_forwarder_reader_info_sync(&self, endpoint_id: &str, info: ReaderInfo) {
        let stream_id = decode_stream_id(info.stream_id);
        let reader_info =
            parse_reader_info_json(info.reader_info_json.as_deref(), endpoint_id, &stream_id);
        let hardware = reader_info.as_ref().and_then(|info| info.hardware.as_ref());
        let hardware_reader_id = optional_non_empty(info.hardware_reader_id)
            .or_else(|| hardware.and_then(|h| h.reader_id.clone()));
        let firmware_version = optional_non_empty(info.firmware_version)
            .or_else(|| hardware.and_then(|h| h.fw_version.clone()));
        let model =
            optional_non_empty(info.model).or_else(|| hardware.and_then(|h| h.hw_code.clone()));
        let mut live_statuses = self.forwarder_live_status.lock().unwrap();
        let live_status = live_statuses.entry(endpoint_id.to_owned()).or_default();
        live_status
            .readers
            .entry(stream_id.clone())
            .and_modify(|reader| {
                if hardware_reader_id.is_some() {
                    reader.hardware_reader_id = hardware_reader_id.clone();
                }
                if firmware_version.is_some() {
                    reader.firmware_version = firmware_version.clone();
                }
                if model.is_some() {
                    reader.model = model.clone();
                }
                if reader_info.is_some() {
                    reader.reader_info = reader_info.clone();
                }
            })
            .or_insert_with(|| ReaderLiveStatus {
                stream_id,
                connected: false,
                state: "unknown".to_owned(),
                last_read_unix_ms: None,
                hardware_reader_id,
                firmware_version,
                model,
                reader_info,
                download_progress: None,
                local_port: None,
            });
    }

    #[cfg(test)]
    pub(crate) async fn record_forwarder_download_progress(
        &self,
        endpoint_id: &str,
        progress: DownloadProgress,
    ) {
        self.store_forwarder_download_progress_sync(endpoint_id, progress);
        self.recompute_aggregate_connection_state().await;
    }

    /// Quick, lock-only store of a reader's download progress with NO aggregate
    /// recompute (see [`Self::store_forwarder_reader_status_sync`]).
    pub(crate) fn store_forwarder_download_progress_sync(
        &self,
        endpoint_id: &str,
        progress: DownloadProgress,
    ) {
        let stream_id = decode_stream_id(progress.stream_id);
        let progress_update = rt_domain::DownloadProgressUpdate {
            reader_ip: stream_id.clone(),
            state: download_state_from_str(&progress.state),
            stored_reads: (progress.total != 0).then_some(progress.total as u32),
            downloaded_reads: progress.reads_received,
            progress: progress.progress,
            total: (progress.total != 0).then_some(progress.total),
            last_read_at: None,
            error: optional_non_empty(progress.error),
        };
        let mut live_statuses = self.forwarder_live_status.lock().unwrap();
        let live_status = live_statuses.entry(endpoint_id.to_owned()).or_default();
        live_status
            .readers
            .entry(stream_id.clone())
            .and_modify(|reader| {
                reader.download_progress = Some(progress_update.clone());
            })
            .or_insert_with(|| ReaderLiveStatus {
                stream_id,
                connected: false,
                state: "unknown".to_owned(),
                last_read_unix_ms: None,
                hardware_reader_id: None,
                firmware_version: None,
                model: None,
                reader_info: None,
                download_progress: Some(progress_update),
                local_port: None,
            });
    }

    #[cfg(test)]
    pub(crate) async fn record_forwarder_ups_status(&self, endpoint_id: &str, status: UpsStatus) {
        self.store_forwarder_ups_status_sync(endpoint_id, status);
        self.recompute_aggregate_connection_state().await;
    }

    /// Quick, lock-only store of a forwarder's UPS status with NO aggregate
    /// recompute (see [`Self::store_forwarder_reader_status_sync`]).
    pub(crate) fn store_forwarder_ups_status_sync(&self, endpoint_id: &str, status: UpsStatus) {
        let mut live_statuses = self.forwarder_live_status.lock().unwrap();
        live_statuses.entry(endpoint_id.to_owned()).or_default().ups = Some(UpsStatusPayload {
            on_battery: status.on_battery,
            battery_percent: status.battery_percent,
            runtime_seconds: status.runtime_seconds,
        });
    }

    /// Register the remote-config request channel for a forwarder whose live
    /// control session negotiated `CAP_REMOTE_CONFIG`. Called by the
    /// [`ForwarderConnection`](crate::p2p_forwarder) on control connect.
    pub(crate) fn register_forwarder_config_tx(
        self: &Arc<Self>,
        endpoint_id: &str,
        tx: mpsc::Sender<ConfigCommand>,
    ) -> ForwarderConfigRegistrationGuard {
        self.forwarder_config_tx
            .lock()
            .unwrap()
            .insert(endpoint_id.to_owned(), tx.clone());
        ForwarderConfigRegistrationGuard {
            state: Arc::clone(self),
            endpoint_id: endpoint_id.to_owned(),
            tx,
        }
    }

    /// Drop a forwarder's remote-config channel on control disconnect/stop so
    /// subsequent config commands fail fast instead of hanging on a dead
    /// session.
    fn deregister_forwarder_config_tx(&self, endpoint_id: &str, tx: &mpsc::Sender<ConfigCommand>) {
        let mut registrations = self.forwarder_config_tx.lock().unwrap();
        if registrations
            .get(endpoint_id)
            .is_some_and(|registered| registered.same_channel(tx))
        {
            registrations.remove(endpoint_id);
        }
    }

    pub(crate) fn forwarder_config_tx(
        &self,
        endpoint_id: &str,
    ) -> Option<mpsc::Sender<ConfigCommand>> {
        self.forwarder_config_tx
            .lock()
            .unwrap()
            .get(endpoint_id)
            .cloned()
    }

    /// Whether the forwarder's live session negotiated `CAP_REMOTE_CONFIG`
    /// (mirrors the presence of its registered remote-config channel).
    pub(crate) fn forwarder_remote_config_available(&self, endpoint_id: &str) -> bool {
        self.forwarder_config_tx
            .lock()
            .unwrap()
            .contains_key(endpoint_id)
    }

    /// Endpoint ids that currently have a live remote-config channel, so
    /// [`get_connections`] can list a forwarder whose only notable state is
    /// remote-config availability.
    fn forwarder_config_endpoints(&self) -> Vec<String> {
        self.forwarder_config_tx
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    /// Register the reader-control request channel for a forwarder whose live
    /// control session negotiated `CAP_READER_CONTROL`.
    pub(crate) fn register_forwarder_reader_control_tx(
        self: &Arc<Self>,
        endpoint_id: &str,
        tx: mpsc::Sender<ReaderCommand>,
    ) -> ForwarderReaderControlRegistrationGuard {
        self.forwarder_reader_control_tx
            .lock()
            .unwrap()
            .insert(endpoint_id.to_owned(), tx.clone());
        ForwarderReaderControlRegistrationGuard {
            state: Arc::clone(self),
            endpoint_id: endpoint_id.to_owned(),
            tx,
        }
    }

    fn deregister_forwarder_reader_control_tx(
        &self,
        endpoint_id: &str,
        tx: &mpsc::Sender<ReaderCommand>,
    ) {
        let mut registrations = self.forwarder_reader_control_tx.lock().unwrap();
        if registrations
            .get(endpoint_id)
            .is_some_and(|registered| registered.same_channel(tx))
        {
            registrations.remove(endpoint_id);
        }
    }

    pub(crate) fn forwarder_reader_control_tx(
        &self,
        endpoint_id: &str,
    ) -> Option<mpsc::Sender<ReaderCommand>> {
        self.forwarder_reader_control_tx
            .lock()
            .unwrap()
            .get(endpoint_id)
            .cloned()
    }

    pub(crate) fn forwarder_reader_control_available(&self, endpoint_id: &str) -> bool {
        self.forwarder_reader_control_tx
            .lock()
            .unwrap()
            .contains_key(endpoint_id)
    }

    fn forwarder_reader_control_endpoints(&self) -> Vec<String> {
        self.forwarder_reader_control_tx
            .lock()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    pub(crate) async fn clear_forwarder_live_status(&self, endpoint_id: &str) {
        {
            self.forwarder_live_status
                .lock()
                .unwrap()
                .remove(endpoint_id);
        }
        self.recompute_aggregate_connection_state().await;
    }

    pub(crate) fn recompute_aggregate_connection_state_sync_default_trying(&self) {
        let statuses = self.forwarder_runtime.lock().unwrap().clone();
        let any_connected = statuses
            .values()
            .any(|status| status.control_up || status.data_sessions > 0);
        let any_trying = statuses
            .values()
            .any(|status| !status.control_up && status.data_sessions == 0);
        let next = if any_connected {
            ConnectionState::Connected
        } else if any_trying {
            ConnectionState::Connecting
        } else {
            ConnectionState::Disconnected
        };
        let _ = self.connection_state.send_if_modified(|state| {
            if *state == next {
                false
            } else {
                *state = next;
                true
            }
        });
    }

    pub(crate) async fn recompute_aggregate_connection_state(&self) {
        let statuses = self.forwarder_runtime.lock().unwrap().clone();
        let (intents, subscriptions) = {
            let db = self.db.lock().await;
            let intents = db.load_forwarder_intents().unwrap_or_default();
            let subscriptions = match db.load_stream_subscriptions() {
                Ok(subscriptions) => subscriptions,
                Err(error) => {
                    warn!(error = %error, "failed to load subscriptions for connections fingerprint");
                    Vec::new()
                }
            };
            (intents, subscriptions)
        };
        let any_connected = statuses
            .values()
            .any(|status| status.control_up || status.data_sessions > 0);
        let any_trying = statuses.iter().any(|(endpoint_id, status)| {
            !status.control_up
                && status.data_sessions == 0
                && *intents.get(endpoint_id).unwrap_or(&true)
        });
        let next = if any_connected {
            ConnectionState::Connected
        } else if any_trying {
            ConnectionState::Connecting
        } else {
            ConnectionState::Disconnected
        };
        let discovered = self.discovered_forwarders.read().await.clone();
        let live_statuses = self.forwarder_live_status.lock().unwrap().clone();
        let config_endpoints = self.forwarder_config_endpoints();
        let reader_control_endpoints = self.forwarder_reader_control_endpoints();
        let fingerprint = Self::connections_fingerprint(
            next.clone(),
            &statuses,
            &intents,
            &discovered,
            &subscriptions,
            &live_statuses,
            &config_endpoints,
            &reader_control_endpoints,
        );
        let connections_changed = {
            let mut last = self.last_connections_fingerprint.lock().unwrap();
            if last.as_ref() == Some(&fingerprint) {
                false
            } else {
                *last = Some(fingerprint);
                true
            }
        };
        if connections_changed {
            let _ = self.ui_tx.send(ReceiverUiEvent::ConnectionsChanged);
        }
        self.set_connection_state_if_changed(next).await;
    }

    #[allow(clippy::too_many_arguments)]
    fn connections_fingerprint(
        aggregate_state: ConnectionState,
        statuses: &HashMap<String, ForwarderRuntimeStatus>,
        intents: &HashMap<String, bool>,
        discovered: &DiscoveredForwarders,
        subscriptions: &[StreamSubscription],
        live_statuses: &HashMap<String, ForwarderLiveStatus>,
        config_endpoints: &[String],
        reader_control_endpoints: &[String],
    ) -> ConnectionsFingerprint {
        let mut endpoints: BTreeSet<String> = statuses.keys().cloned().collect();
        endpoints.extend(intents.keys().cloned());
        endpoints.extend(discovered.keys().cloned());
        endpoints.extend(live_statuses.keys().cloned());
        endpoints.extend(config_endpoints.iter().cloned());
        endpoints.extend(reader_control_endpoints.iter().cloned());

        let mut subscribed_counts: HashMap<String, usize> = HashMap::new();
        let local_ports = subscription_local_ports(subscriptions);
        for subscription in subscriptions {
            endpoints.insert(subscription.forwarder_endpoint_id.clone());
            *subscribed_counts
                .entry(subscription.forwarder_endpoint_id.clone())
                .or_default() += 1;
        }

        let forwarders = endpoints
            .into_iter()
            .map(|endpoint_id| {
                let runtime = statuses.get(&endpoint_id).copied().unwrap_or_default();
                let intent = *intents.get(&endpoint_id).unwrap_or(&true);
                let snapshot = derive_forwarder_state(runtime, intent);
                let discovered_forwarder = discovered.get(&endpoint_id);
                let live_status = live_statuses.get(&endpoint_id).cloned().unwrap_or_default();
                ForwarderConnectionsFingerprint {
                    endpoint_id: endpoint_id.clone(),
                    state: snapshot.state,
                    pending: snapshot.pending,
                    intent,
                    discovered: discovered_forwarder.is_some(),
                    display_name: discovered_forwarder
                        .and_then(|forwarder| forwarder.display_name.clone()),
                    remote_config_available: config_endpoints.contains(&endpoint_id),
                    reader_control_available: reader_control_endpoints.contains(&endpoint_id),
                    subscribed_count: subscribed_counts.get(&endpoint_id).copied().unwrap_or(0),
                    available_count: discovered_forwarder
                        .map_or(0, |forwarder| forwarder.streams.len()),
                    readers: sorted_reader_statuses(&live_status, &local_ports, &endpoint_id),
                    ups: live_status.ups,
                }
            })
            .collect();

        ConnectionsFingerprint {
            aggregate_state,
            forwarders,
        }
    }

    async fn set_connection_state_if_changed(&self, new_state: ConnectionState) {
        let changed = self.connection_state.send_if_modified(|state| {
            *state != new_state && {
                *state = new_state.clone();
                true
            }
        });
        if changed {
            self.emit_connection_state_side_effects(new_state).await;
        }
    }

    pub async fn request_retry_connect(&self) {
        self.retry_streak.fetch_add(1, Ordering::SeqCst);
        self.bump_connect_attempt(None, true);
        self.set_connection_state(ConnectionState::Connecting).await;
    }

    pub async fn request_reconnect_if_connected(&self) -> bool {
        let was_connected = self.connection_state.send_if_modified(|state| {
            if *state == ConnectionState::Connected {
                *state = ConnectionState::Connecting;
                true
            } else {
                false
            }
        });
        if !was_connected {
            return false;
        }
        self.retry_streak.fetch_add(1, Ordering::SeqCst);
        self.bump_connect_attempt(None, true);
        self.emit_connection_state_side_effects(ConnectionState::Connecting)
            .await;
        true
    }

    pub(crate) async fn emit_connection_state_side_effects(&self, new_state: ConnectionState) {
        let streams_count = {
            let db = self.db.lock().await;
            match db.load_stream_subscriptions() {
                Ok(s) => s.len(),
                Err(e) => {
                    warn!(error = %e, "failed to load subscriptions for status event");
                    0
                }
            }
        };
        let receiver_id = self.receiver_id.read().await.clone();
        let _ = self.ui_tx.send(ReceiverUiEvent::StatusChanged {
            connection_state: new_state.clone(),
            streams_count,
            receiver_id,
        });
        let label = match &new_state {
            ConnectionState::Disconnected => "Disconnected",
            ConnectionState::Connecting => "Connecting",
            ConnectionState::Connected => "Connected",
            ConnectionState::Disconnecting => "Disconnecting",
        };
        self.logger.log(label);
    }

    /// Update connection state, broadcast status change, and emit a log entry.
    pub async fn set_connection_state(&self, new_state: ConnectionState) {
        let _ = self.connection_state.send(new_state.clone());
        self.emit_connection_state_side_effects(new_state).await;
    }

    /// Build the streams response from durable local subscriptions and cursors.
    pub async fn build_streams_response(&self) -> StreamsResponse {
        let counts_snapshot = self.stream_counts.snapshot();
        let db = self.db.lock().await;
        let subs = match db.load_stream_subscriptions() {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to load subscriptions for streams response");
                return StreamsResponse {
                    streams: vec![],
                    degraded: true,
                    upstream_error: Some(format!("failed to load subscriptions: {e}")),
                };
            }
        };
        let (cursors, cursors_degraded) = match db.load_stream_cursors() {
            Ok(c) => (c, false),
            Err(e) => {
                warn!(error = %e, "failed to load cursors");
                (vec![], true)
            }
        };
        let announcer_publish_streams = db.load_announcer_publish_streams().unwrap_or_default();
        let intents = db.load_forwarder_intents().unwrap_or_default();
        drop(db);

        let runtime_statuses = self.forwarder_runtime.lock().unwrap().clone();
        let live_statuses = self.forwarder_live_status.lock().unwrap().clone();

        let cursor_map: HashMap<&str, &crate::db::StreamCursorRecord> =
            cursors.iter().map(|c| (c.stream_id.as_str(), c)).collect();
        let discovered = self.discovered_forwarders.read().await;
        let discovered_streams: HashMap<(&str, &str), (&DiscoveredForwarder, &DiscoveredStream)> =
            discovered
                .iter()
                .flat_map(|(endpoint_id, forwarder)| {
                    forwarder.streams.iter().map(move |stream| {
                        (
                            (endpoint_id.as_str(), stream.stream_id.as_str()),
                            (forwarder, stream),
                        )
                    })
                })
                .collect();

        let mut streams: Vec<StreamEntry> = Vec::new();

        for sub in &subs {
            let display_reader_ip = sub.reader_ip.clone().or_else(|| {
                crate::ports::reader_addr_if_port_mappable(&sub.stream_id)
                    .map(std::borrow::ToOwned::to_owned)
            });
            let display_forwarder_id = sub.forwarder_id.clone().or_else(|| {
                display_reader_ip
                    .as_ref()
                    .map(|_| sub.forwarder_endpoint_id.clone())
            });
            let port = sub.local_port_override.or_else(|| {
                display_reader_ip
                    .as_deref()
                    .and_then(crate::ports::default_port)
            });
            let counts = display_forwarder_id
                .as_deref()
                .zip(display_reader_ip.as_deref())
                .and_then(|(forwarder_id, reader_ip)| {
                    let sk = crate::cache::StreamKey::new(forwarder_id, reader_ip);
                    counts_snapshot.get(&sk)
                });
            let cursor = cursor_map.get(sub.stream_id.as_str());
            let discovered_stream = discovered_streams
                .get(&(sub.forwarder_endpoint_id.as_str(), sub.stream_id.as_str()));
            let runtime = runtime_statuses
                .get(&sub.forwarder_endpoint_id)
                .copied()
                .unwrap_or_default();
            let intent = *intents.get(&sub.forwarder_endpoint_id).unwrap_or(&true);
            let snapshot = derive_forwarder_state(runtime, intent);
            let online = Some(matches!(
                snapshot.state,
                ForwarderConnState::Connected | ForwarderConnState::Subscribed
            ));
            let reader_connected = live_statuses
                .get(&sub.forwarder_endpoint_id)
                .and_then(|status| status.readers.get(&sub.stream_id))
                .map(|reader| reader.connected)
                .or_else(|| (snapshot.state == ForwarderConnState::Subscribed).then_some(true));
            streams.push(StreamEntry {
                forwarder_endpoint_id: sub.forwarder_endpoint_id.clone(),
                stream_id: sub.stream_id.clone(),
                forwarder_id: display_forwarder_id,
                reader_ip: display_reader_ip,
                subscribed: true,
                local_port: port,
                announcer_publish: announcer_publish_streams.contains(&sub.stream_id),
                event_type: Some(sub.event_type),
                online,
                reader_connected,
                display_alias: discovered_stream
                    .and_then(|(forwarder, _)| forwarder.display_name.clone()),
                stream_epoch: discovered_stream.map(|(_, stream)| stream.epoch),
                current_epoch_name: None,
                reads_total: counts.as_ref().map(|c| c.total),
                reads_epoch: counts.as_ref().map(|c| c.epoch),
                cursor_epoch: cursor.and_then(|c| c.stream_epoch),
                cursor_seq: cursor.map(|c| c.last_seq),
            });
        }

        // Append discovered-but-unsubscribed streams as `subscribed = false`
        // so the UI can list streams that are available to subscribe to. A
        // (forwarder_endpoint_id, stream_id) already present as a subscription
        // takes precedence and is not duplicated.
        let mut seen: std::collections::HashSet<(String, String)> = streams
            .iter()
            .map(|s| (s.forwarder_endpoint_id.clone(), s.stream_id.clone()))
            .collect();
        for (endpoint_id, forwarder) in discovered.iter() {
            for stream in &forwarder.streams {
                let key = (endpoint_id.clone(), stream.stream_id.clone());
                if !seen.insert(key) {
                    continue;
                }
                let runtime = runtime_statuses
                    .get(endpoint_id)
                    .copied()
                    .unwrap_or_default();
                let intent = *intents.get(endpoint_id).unwrap_or(&true);
                let snapshot = derive_forwarder_state(runtime, intent);
                let online = Some(matches!(
                    snapshot.state,
                    ForwarderConnState::Connected | ForwarderConnState::Subscribed
                ));
                let reader_connected = live_statuses
                    .get(endpoint_id)
                    .and_then(|status| status.readers.get(&stream.stream_id))
                    .map(|reader| reader.connected)
                    .or_else(|| (snapshot.state == ForwarderConnState::Subscribed).then_some(true));
                streams.push(StreamEntry {
                    forwarder_endpoint_id: endpoint_id.clone(),
                    stream_id: stream.stream_id.clone(),
                    forwarder_id: None,
                    reader_ip: None,
                    subscribed: false,
                    local_port: None,
                    announcer_publish: false,
                    event_type: None,
                    online,
                    reader_connected,
                    display_alias: forwarder.display_name.clone(),
                    stream_epoch: Some(stream.epoch),
                    current_epoch_name: None,
                    reads_total: None,
                    reads_epoch: None,
                    cursor_epoch: None,
                    cursor_seq: None,
                });
            }
        }
        drop(discovered);

        let degraded = cursors_degraded;
        let upstream_error = cursors_degraded.then(|| "failed to load cursors".to_owned());
        StreamsResponse {
            streams,
            degraded,
            upstream_error,
        }
    }

    /// Build and broadcast a streams snapshot to UI clients.
    pub async fn emit_streams_snapshot(&self) {
        let response = self.build_streams_response().await;
        let _ = self.ui_tx.send(ReceiverUiEvent::StreamsSnapshot {
            streams: response.streams,
            degraded: response.degraded,
            upstream_error: response.upstream_error,
        });
    }

    /// Ask UI clients to reload full state from the control API.
    pub fn emit_resync(&self) {
        let _ = self.ui_tx.send(ReceiverUiEvent::Resync);
    }
}

/// RAII registration for a forwarder's live remote-config command channel.
/// Dropping the guard deregisters the channel even if the owning connection
/// task is aborted or panics.
pub(crate) struct ForwarderConfigRegistrationGuard {
    state: Arc<AppState>,
    endpoint_id: String,
    tx: mpsc::Sender<ConfigCommand>,
}

impl Drop for ForwarderConfigRegistrationGuard {
    fn drop(&mut self) {
        self.state
            .deregister_forwarder_config_tx(&self.endpoint_id, &self.tx);
        self.state
            .recompute_aggregate_connection_state_sync_default_trying();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = Arc::clone(&self.state);
            handle.spawn(async move {
                state.recompute_aggregate_connection_state().await;
            });
        }
    }
}

/// RAII registration for a forwarder's live reader-control command channel.
pub(crate) struct ForwarderReaderControlRegistrationGuard {
    state: Arc<AppState>,
    endpoint_id: String,
    tx: mpsc::Sender<ReaderCommand>,
}

impl Drop for ForwarderReaderControlRegistrationGuard {
    fn drop(&mut self) {
        self.state
            .deregister_forwarder_reader_control_tx(&self.endpoint_id, &self.tx);
        self.state
            .recompute_aggregate_connection_state_sync_default_trying();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let state = Arc::clone(&self.state);
            handle.spawn(async move {
                state.recompute_aggregate_connection_state().await;
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Request/Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
pub struct ProfileRequest {
    pub server_url: String,
    pub token: String,
    #[serde(default)]
    pub receiver_id: Option<String>,
}

fn is_valid_receiver_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn is_uuid_format(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }

    value.bytes().enumerate().all(|(index, byte)| match index {
        8 | 13 | 18 | 23 => byte == b'-',
        _ => byte.is_ascii_hexdigit(),
    })
}

#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    /// Effective server URL (resolved: env override > profile).
    pub server_url: String,
    /// Effective server token (resolved: env override > profile).
    pub token: String,
    pub receiver_id: String,
    /// Where the effective server config comes from: `"env"` (RT_P2P_SERVER_*
    /// override active), `"profile"` (stored profile), or `"none"`. The UI
    /// renders the URL/token read-only when this is `"env"`.
    pub server_source: String,
    /// Global announcer publish toggle state.
    pub announcer_enabled: bool,
    /// Receiver-configured cap on visible rows in the server announcer feed.
    pub announcer_max_list_size: u32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubscriptionRequest {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    pub local_port_override: Option<u16>,
    pub event_type: Option<crate::db::EventType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarder_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_ip: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubscriptionsBody {
    pub subscriptions: Vec<SubscriptionRequest>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CursorResetRequest {
    pub stream_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdatePortRequest {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    pub local_port_override: Option<u16>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StreamEntry {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarder_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader_ip: Option<String>,
    pub subscribed: bool,
    pub local_port: Option<u16>,
    /// Whether this stream is opted in to announcer publishing.
    pub announcer_publish: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_type: Option<crate::db::EventType>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader_connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_epoch: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_epoch_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reads_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reads_epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_epoch: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_seq: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StreamsResponse {
    pub streams: Vec<StreamEntry>,
    pub degraded: bool,
    pub upstream_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerDeviceStatus {
    pub configured: bool,
    pub endpoint_id: Option<String>,
    pub reachable: Option<bool>,
    pub approval_state: Option<String>,
    pub waiting_for_approval: bool,
    pub message: Option<String>,
}

impl ServerDeviceStatus {
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

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub receiver_id: String,
    pub connection_state: ConnectionState,
    pub local_ok: bool,
    pub streams_count: usize,
    pub server: ServerDeviceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReaderLiveStatus {
    pub stream_id: String,
    pub connected: bool,
    pub state: String,
    pub last_read_unix_ms: Option<i64>,
    pub hardware_reader_id: Option<String>,
    pub firmware_version: Option<String>,
    pub model: Option<String>,
    pub reader_info: Option<rt_domain::ReaderInfo>,
    pub download_progress: Option<rt_domain::DownloadProgressUpdate>,
    pub local_port: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpsStatusPayload {
    pub on_battery: bool,
    pub battery_percent: u32,
    pub runtime_seconds: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForwarderConnectionStatus {
    pub endpoint_id: String,
    pub display_name: Option<String>,
    pub state: ForwarderConnState,
    pub pending: bool,
    pub subscribed_count: usize,
    pub available_count: usize,
    pub readers: Vec<ReaderLiveStatus>,
    pub ups: Option<UpsStatusPayload>,
    pub restart_needed: Option<bool>,
    /// `true` only when this forwarder has a live control session that
    /// negotiated `CAP_REMOTE_CONFIG`; gates the UI's view/edit/restart
    /// affordances.
    pub remote_config_available: bool,
    pub reader_control_available: bool,
}

/// Result of [`get_forwarder_config`]: the forwarder's full config document and
/// whether applying the currently-persisted config requires a restart.
#[derive(Debug, Clone, Serialize)]
pub struct ForwarderConfigResponse {
    pub config_json: String,
    pub restart_needed: bool,
}

/// Result of [`set_forwarder_config`].
#[derive(Debug, Clone, Serialize)]
pub struct ForwarderConfigSetResult {
    pub ok: bool,
    pub restart_needed: bool,
    pub error: Option<String>,
}

/// Result of [`restart_forwarder`].
#[derive(Debug, Clone, Serialize)]
pub struct ForwarderRestartResult {
    pub accepted: bool,
    pub error: Option<String>,
}

/// Result of a reader-control command proxied to a forwarder over P2P.
#[derive(Debug, Clone, Serialize)]
pub struct ReaderControlResult {
    pub success: bool,
    pub message: String,
    pub reader_info: Option<rt_domain::ReaderInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionsResponse {
    pub server: ServerDeviceStatus,
    pub forwarders: Vec<ForwarderConnectionStatus>,
}

#[derive(Debug, Serialize)]
pub struct LogsResponse {
    pub entries: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EarliestEpochRequest {
    pub forwarder_endpoint_id: String,
    pub stream_id: String,
    pub earliest_epoch: i64,
}

#[derive(Debug, Serialize)]
pub struct ReplayTargetEpochOption {
    pub stream_epoch: i64,
    pub name: Option<String>,
    pub first_seen_at: Option<String>,
    pub race_names: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct ReplayTargetEpochsResponse {
    pub epochs: Vec<ReplayTargetEpochOption>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventTypeRequest {
    pub event_type: crate::db::EventType,
}

// ---------------------------------------------------------------------------
// Handler functions (plain async, no Axum)
// ---------------------------------------------------------------------------

/// Summary returned by the participant/chip import commands.
#[derive(Debug, Serialize)]
pub struct ImportSummary {
    /// Rows accepted into the table (participants or chip assignments).
    pub imported: usize,
    /// Chips that resolve to a participant after the import (post-join).
    pub resolvable_chips: usize,
}

fn decode_import_bytes(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => decode_windows_1252(e.into_bytes()),
    }
}

fn decode_windows_1252(bytes: Vec<u8>) -> String {
    bytes
        .into_iter()
        .map(|b| match b {
            0x80 => '\u{20ac}',
            0x82 => '\u{201a}',
            0x83 => '\u{0192}',
            0x84 => '\u{201e}',
            0x85 => '\u{2026}',
            0x86 => '\u{2020}',
            0x87 => '\u{2021}',
            0x88 => '\u{02c6}',
            0x89 => '\u{2030}',
            0x8a => '\u{0160}',
            0x8b => '\u{2039}',
            0x8c => '\u{0152}',
            0x8e => '\u{017d}',
            0x91 => '\u{2018}',
            0x92 => '\u{2019}',
            0x93 => '\u{201c}',
            0x94 => '\u{201d}',
            0x95 => '\u{2022}',
            0x96 => '\u{2013}',
            0x97 => '\u{2014}',
            0x98 => '\u{02dc}',
            0x99 => '\u{2122}',
            0x9a => '\u{0161}',
            0x9b => '\u{203a}',
            0x9c => '\u{0153}',
            0x9e => '\u{017e}',
            0x9f => '\u{0178}',
            _ => char::from(b),
        })
        .collect()
}

async fn read_import_file(path: String) -> Result<String, ReceiverError> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ReceiverError::BadRequest(format!("failed to read import file: {e}")))?;
    Ok(decode_import_bytes(bytes))
}

/// Import participants from `.ppl` file contents. Strict: a parse error rejects
/// the whole file and leaves the existing table untouched. On success the table
/// is replaced wholesale and the chip lookup is rebuilt.
pub async fn import_participants(
    state: &AppState,
    contents: String,
) -> Result<ImportSummary, ReceiverError> {
    let participants =
        crate::participants::parse_ppl(&contents).map_err(ReceiverError::BadRequest)?;
    let imported = participants.len();
    {
        let mut db = state.db.lock().await;
        db.replace_participants(&participants)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    let resolvable_chips = reload_chip_lookup(state).await?;
    Ok(ImportSummary {
        imported,
        resolvable_chips,
    })
}

/// Import participants from a `.ppl` file path selected by the desktop UI.
pub async fn import_participants_file(
    state: &AppState,
    path: String,
) -> Result<ImportSummary, ReceiverError> {
    import_participants(state, read_import_file(path).await?).await
}

/// Return current participant/chip counts and how they overlap, so the UI can
/// show data state without an import round-trip.
pub async fn get_data_stats(state: &AppState) -> Result<crate::db::DataStats, ReceiverError> {
    let db = state.db.lock().await;
    db.data_stats()
        .map_err(|e| ReceiverError::Internal(e.to_string()))
}

/// Import bib->chip assignments from `.bibchip` file contents. Strict, like
/// [`import_participants`].
pub async fn import_chips(
    state: &AppState,
    contents: String,
) -> Result<ImportSummary, ReceiverError> {
    let chips = crate::participants::parse_bibchip(&contents).map_err(ReceiverError::BadRequest)?;
    let imported = chips.len();
    {
        let mut db = state.db.lock().await;
        db.replace_bib_chips(&chips)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    let resolvable_chips = reload_chip_lookup(state).await?;
    Ok(ImportSummary {
        imported,
        resolvable_chips,
    })
}

/// Import bib->chip assignments from a `.bibchip` file path selected by the
/// desktop UI.
pub async fn import_chips_file(
    state: &AppState,
    path: String,
) -> Result<ImportSummary, ReceiverError> {
    import_chips(state, read_import_file(path).await?).await
}

/// Enable or disable the global announcer publish toggle. The P2P reconcile
/// loop picks up the change on its next pass (within one reconcile interval).
pub async fn set_announcer_enabled(state: &AppState, enabled: bool) -> Result<(), ReceiverError> {
    {
        let db = state.db.lock().await;
        db.set_announcer_enabled(enabled)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    // Other UI/bridge clients hold this in profile state; nudge them to refetch.
    state.emit_resync();
    Ok(())
}

/// Set the receiver-configured cap on visible rows in the server announcer
/// feed. The value is clamped to `1..=500` and rides the next announcer push to
/// the server. The P2P reconcile loop picks up the change on its next pass.
pub async fn set_announcer_max_list_size(
    state: &AppState,
    max_list_size: u32,
) -> Result<(), ReceiverError> {
    let clamped = max_list_size.clamp(1, 500);
    {
        let db = state.db.lock().await;
        db.set_announcer_max_list_size(clamped)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    // Other UI/bridge clients hold this in profile state; nudge them to refetch.
    state.emit_resync();
    Ok(())
}

/// Opt a single stream in/out of announcer publishing (opt-in default off).
pub async fn set_stream_announcer_publish(
    state: &AppState,
    stream_id: &str,
    publish: bool,
) -> Result<(), ReceiverError> {
    {
        let db = state.db.lock().await;
        db.set_stream_announcer_publish(stream_id, publish)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    // The per-stream publish flag rides on the streams snapshot, so broadcast
    // it for other clients (SSE-only; does not restart stream workers).
    state.emit_streams_snapshot().await;
    Ok(())
}

/// Rebuild the in-memory chip->participant lookup from the durable
/// participant/chip tables. Called at startup and after each import. Returns
/// the number of resolvable chips. The lookup uses a single outer key
/// (`"default"`); the announcer resolver searches across all outer maps.
pub async fn reload_chip_lookup(state: &AppState) -> Result<usize, ReceiverError> {
    let map = {
        let db = state.db.lock().await;
        db.load_chip_to_participant()
            .map_err(|e| ReceiverError::Internal(e.to_string()))?
    };
    let count = map.len();
    let mut lookup = state.chip_lookup.write().await;
    lookup.clear();
    lookup.insert("default".to_owned(), map);
    Ok(count)
}

/// Choose the server URL+token to persist on a profile save.
///
/// When a server env override is active the UI locks the URL/token inputs and
/// `get_profile` returns the effective (env) values, which the client echoes
/// back on save. Persisting those would copy the env token into the profile
/// DB, so the stored values are preserved instead. Otherwise the request body
/// (already trimmed for the URL) is persisted.
fn server_fields_to_persist(
    env_active: bool,
    body_url: String,
    body_token: String,
    existing: Option<&crate::db::Profile>,
) -> (String, String) {
    if env_active {
        existing.map_or((String::new(), String::new()), |p| {
            (p.server_url.clone(), p.token.clone())
        })
    } else {
        (body_url, body_token)
    }
}

/// Whether a server URL+token override is active (both set and non-empty).
/// When active, the stored profile server fields are read-only in the UI and
/// must not be overwritten by a profile save.
fn server_override_active(override_: &(Option<String>, Option<String>)) -> bool {
    let non_empty = |s: &Option<String>| s.as_deref().is_some_and(|v| !v.trim().is_empty());
    non_empty(&override_.0) && non_empty(&override_.1)
}

pub async fn get_profile(state: &AppState) -> Result<ProfileResponse, ReceiverError> {
    let receiver_id = state.receiver_id.read().await.clone();
    let profile = {
        let db = state.db.lock().await;
        db.load_profile()
            .map_err(|e| ReceiverError::Internal(e.to_string()))?
    };

    // Report the effective server config and its source so the UI can show the
    // real values (and lock the fields) when an override is active.
    let override_ = state.server_override().await;
    let env_active = server_override_active(&override_);
    let resolved = crate::runtime::resolve_server_config(profile.as_ref(), override_);
    let server_source = if env_active {
        "env"
    } else if resolved.is_some() {
        "profile"
    } else {
        "none"
    }
    .to_owned();
    let (server_url, token) =
        resolved.map_or_else(|| (String::new(), String::new()), |s| (s.url, s.token));
    let (announcer_enabled, announcer_max_list_size) = {
        let db = state.db.lock().await;
        (
            db.load_announcer_enabled().unwrap_or(false),
            db.load_announcer_max_list_size().unwrap_or(25),
        )
    };
    Ok(ProfileResponse {
        server_url,
        token,
        receiver_id,
        server_source,
        announcer_enabled,
        announcer_max_list_size,
    })
}

pub async fn get_mode(state: &AppState) -> Result<ReceiverMode, ReceiverError> {
    let db = state.db.lock().await;
    match db.load_receiver_mode() {
        Ok(Some(mode)) => Ok(mode),
        Ok(None) => Err(ReceiverError::NotFound("no mode configured".to_owned())),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn put_profile(state: &AppState, body: ProfileRequest) -> Result<(), ReceiverError> {
    let url = body.server_url.trim().trim_end_matches('/').to_owned();

    let new_receiver_id = body
        .receiver_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned);

    if let Some(ref id) = new_receiver_id
        && !is_valid_receiver_id(id)
    {
        return Err(ReceiverError::BadRequest(
            "receiver_id must be 1-64 characters, alphanumeric/hyphens/underscores only".to_owned(),
        ));
    }

    let mut db = state.db.lock().await;
    let existing = db.load_profile().ok().flatten();
    let persist_receiver_id = new_receiver_id
        .clone()
        .or_else(|| existing.as_ref().and_then(|p| p.receiver_id.clone()));

    let (persist_url, persist_token) = server_fields_to_persist(
        server_override_active(&state.server_override().await),
        url,
        body.token.clone(),
        existing.as_ref(),
    );

    match db.save_profile(
        &persist_url,
        &persist_token,
        DEFAULT_UPDATE_MODE,
        persist_receiver_id.as_deref(),
    ) {
        Ok(()) => {
            drop(db);
            if let Some(id) = new_receiver_id {
                *state.receiver_id.write().await = id;
            }
            // The server URL+token may have changed; signal the P2P reconcile
            // loop to rebind its server-bound tasks (register/takeover,
            // discovery, announcer).
            state.notify_server_config_changed();
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn put_mode(state: &AppState, mode: ReceiverMode) -> Result<(), ReceiverError> {
    if let ReceiverMode::Race { race_id } = &mode {
        if race_id.trim().is_empty() {
            return Err(ReceiverError::BadRequest(
                "race_id must not be empty when mode is race".to_owned(),
            ));
        }
        if !is_uuid_format(race_id) {
            return Err(ReceiverError::BadRequest(
                "race_id must be a valid UUID when mode is race".to_owned(),
            ));
        }
    }

    let db = state.db.lock().await;
    match db.save_receiver_mode(&mode) {
        Ok(()) => {
            drop(db);
            let _ = state
                .ui_tx
                .send(crate::ui_events::ReceiverUiEvent::ModeChanged { mode: mode.clone() });
            state.emit_streams_snapshot().await;
            state.request_connect().await;
            Ok(())
        }
        Err(crate::db::DbError::ProfileMissing) => {
            Err(ReceiverError::NotFound("no profile".to_owned()))
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn put_earliest_epoch(
    state: &AppState,
    body: EarliestEpochRequest,
) -> Result<(), ReceiverError> {
    if body.earliest_epoch < 0 {
        return Err(ReceiverError::BadRequest(
            "earliest_epoch must be a non-negative integer".to_owned(),
        ));
    }

    let db = state.db.lock().await;
    match db.save_stream_earliest_epoch(
        &body.forwarder_endpoint_id,
        &body.stream_id,
        body.earliest_epoch,
    ) {
        Ok(()) => {
            drop(db);
            let _ = state.request_reconnect_if_connected().await;
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn get_streams(state: &AppState) -> StreamsResponse {
    state.build_streams_response().await
}

pub async fn get_stream_metrics(state: &AppState) -> Vec<crate::ui_events::StreamMetricsPayload> {
    state.get_stream_metrics_snapshot().await
}

pub async fn get_replay_target_epochs(
    state: &AppState,
    stream_id: String,
) -> Result<ReplayTargetEpochsResponse, ReceiverError> {
    let db = state.db.lock().await;
    let rows = db
        .load_replay_target_epochs(&stream_id)
        .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    Ok(ReplayTargetEpochsResponse {
        epochs: rows
            .into_iter()
            .map(|(stream_epoch, first_seen_at)| ReplayTargetEpochOption {
                stream_epoch,
                name: None,
                first_seen_at,
                race_names: Vec::new(),
            })
            .collect(),
    })
}

pub async fn put_subscriptions(
    state: &AppState,
    body: SubscriptionsBody,
) -> Result<(), ReceiverError> {
    let mut seen = std::collections::HashSet::new();
    for s in &body.subscriptions {
        if !seen.insert((s.forwarder_endpoint_id.clone(), s.stream_id.clone())) {
            return Err(ReceiverError::BadRequest(
                "duplicate subscriptions".to_owned(),
            ));
        }
    }

    let subs: Vec<StreamSubscription> = body
        .subscriptions
        .into_iter()
        .map(|s| StreamSubscription {
            forwarder_endpoint_id: s.forwarder_endpoint_id,
            stream_id: s.stream_id,
            local_port_override: s.local_port_override,
            event_type: s.event_type.unwrap_or(crate::db::EventType::Finish),
            forwarder_id: s.forwarder_id,
            reader_ip: s.reader_ip,
        })
        .collect();
    let mut db = state.db.lock().await;
    match db.replace_stream_subscriptions(&subs) {
        Ok(()) => {
            drop(db);
            let conn_for_status = state.connection_state.borrow().clone();
            let db = state.db.lock().await;
            let streams_count = db.load_stream_subscriptions().map(|s| s.len()).unwrap_or(0);
            let receiver_id = state.receiver_id.read().await.clone();
            let _ = state.ui_tx.send(ReceiverUiEvent::StatusChanged {
                connection_state: conn_for_status,
                streams_count,
                receiver_id,
            });
            drop(db);
            state.emit_streams_snapshot().await;
            let conn_for_reconnect = state.connection_state.borrow().clone();
            if matches!(
                conn_for_reconnect,
                ConnectionState::Connected
                    | ConnectionState::Connecting
                    | ConnectionState::Disconnected
            ) {
                state.request_connect().await;
            }
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn get_subscriptions(state: &AppState) -> Result<SubscriptionsBody, ReceiverError> {
    let db = state.db.lock().await;
    match db.load_stream_subscriptions() {
        Ok(subscriptions) => Ok(SubscriptionsBody {
            subscriptions: subscriptions
                .into_iter()
                .map(|s| SubscriptionRequest {
                    forwarder_endpoint_id: s.forwarder_endpoint_id,
                    stream_id: s.stream_id,
                    local_port_override: s.local_port_override,
                    event_type: Some(s.event_type),
                    forwarder_id: s.forwarder_id,
                    reader_ip: s.reader_ip,
                })
                .collect(),
        }),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn get_status(state: &AppState) -> StatusResponse {
    let receiver_id = state.receiver_id.read().await.clone();
    let conn = state.connection_state.borrow().clone();
    let db = state.db.lock().await;
    let streams_count = db.load_stream_subscriptions().map(|s| s.len()).unwrap_or(0);
    let local_ok = state.db_integrity_ok;
    drop(db);
    let server = server_device_status(state).await;
    StatusResponse {
        receiver_id,
        connection_state: conn,
        local_ok,
        streams_count,
        server,
    }
}

pub async fn get_connections(state: &AppState) -> ConnectionsResponse {
    let server = server_device_status(state).await;
    let discovered = state.discovered_forwarders.read().await.clone();
    let subscriptions = {
        let db = state.db.lock().await;
        match db.load_stream_subscriptions() {
            Ok(subscriptions) => subscriptions,
            Err(error) => {
                warn!(error = %error, "failed to load subscriptions for connections response");
                Vec::new()
            }
        }
    };

    let live_statuses = state.forwarder_live_status.lock().unwrap().clone();
    let mut endpoints: BTreeSet<String> = discovered.keys().cloned().collect();
    endpoints.extend(live_statuses.keys().cloned());
    endpoints.extend(state.forwarder_config_endpoints());
    endpoints.extend(state.forwarder_reader_control_endpoints());
    let mut subscribed_counts: HashMap<String, usize> = HashMap::new();
    let local_ports = subscription_local_ports(&subscriptions);
    for subscription in &subscriptions {
        endpoints.insert(subscription.forwarder_endpoint_id.clone());
        *subscribed_counts
            .entry(subscription.forwarder_endpoint_id.clone())
            .or_default() += 1;
    }

    let mut forwarders = Vec::with_capacity(endpoints.len());
    for endpoint_id in endpoints {
        let discovered_forwarder = discovered.get(&endpoint_id);
        let snapshot = state.forwarder_state(&endpoint_id).await;
        let live_status = live_statuses.get(&endpoint_id).cloned().unwrap_or_default();
        forwarders.push(ForwarderConnectionStatus {
            endpoint_id: endpoint_id.clone(),
            display_name: discovered_forwarder.and_then(|forwarder| forwarder.display_name.clone()),
            state: snapshot.state,
            pending: snapshot.pending,
            subscribed_count: subscribed_counts.get(&endpoint_id).copied().unwrap_or(0),
            available_count: discovered_forwarder.map_or(0, |forwarder| forwarder.streams.len()),
            readers: sorted_reader_statuses(&live_status, &local_ports, &endpoint_id),
            ups: live_status.ups,
            restart_needed: None,
            remote_config_available: state.forwarder_remote_config_available(&endpoint_id),
            reader_control_available: state.forwarder_reader_control_available(&endpoint_id),
        });
    }

    ConnectionsResponse { server, forwarders }
}

#[derive(Debug, Deserialize)]
struct ServerStatusBoard {
    #[serde(default)]
    devices: Vec<ServerStatusDevice>,
}

#[derive(Debug, Deserialize)]
struct ServerStatusDevice {
    endpoint_id: String,
    approval_state: String,
}

pub(crate) async fn server_device_status(state: &AppState) -> ServerDeviceStatus {
    let server_url = {
        let profile = {
            let db = state.db.lock().await;
            db.load_profile().ok().flatten()
        };
        // Mirror the P2P runtime's resolution (env/CLI override > profile) so
        // the status card reflects the server the receiver actually targets.
        match crate::runtime::resolve_server_config(profile.as_ref(), state.server_override().await)
        {
            Some(server) => server.url,
            None => return ServerDeviceStatus::not_configured(),
        }
    };

    server_device_status_for_url(state, &server_url).await
}

pub(crate) async fn server_device_status_for_url(
    state: &AppState,
    server_url: &str,
) -> ServerDeviceStatus {
    let endpoint_id = state.p2p_endpoint_id.read().await.clone();

    let Some(endpoint_id) = endpoint_id else {
        return ServerDeviceStatus {
            configured: true,
            endpoint_id: None,
            reachable: None,
            approval_state: None,
            waiting_for_approval: true,
            message: Some("Waiting for the local P2P endpoint to start".to_owned()),
        };
    };

    let status_url = format!("{}/status", server_url.trim_end_matches('/'));
    let response = match state.http_client.get(status_url).send().await {
        Ok(response) => response,
        Err(error) => {
            return ServerDeviceStatus {
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
        Ok(response) => match response.json::<ServerStatusBoard>().await {
            Ok(board) => board,
            Err(error) => {
                return ServerDeviceStatus {
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
            return ServerDeviceStatus {
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
            ServerDeviceStatus {
                configured: true,
                endpoint_id: Some(endpoint_id),
                reachable: Some(true),
                approval_state: Some(device.approval_state),
                waiting_for_approval,
                message: waiting_for_approval
                    .then(|| "Waiting for server admin approval".to_owned()),
            }
        }
        None => ServerDeviceStatus {
            configured: true,
            endpoint_id: Some(endpoint_id),
            reachable: Some(true),
            approval_state: None,
            waiting_for_approval: true,
            message: Some("Waiting for this receiver to register with the server".to_owned()),
        },
    }
}

pub async fn reconnect_server(state: &AppState) -> Result<(), ReceiverError> {
    state.request_connect().await;
    state.emit_resync();
    Ok(())
}

pub async fn connect_forwarder(state: &AppState, endpoint_id: String) -> Result<(), ReceiverError> {
    {
        let db = state.db.lock().await;
        db.set_forwarder_intent(&endpoint_id, true)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    state.recompute_aggregate_connection_state().await;
    state.wake_reconcile();
    state.emit_resync();
    Ok(())
}

pub async fn disconnect_forwarder(
    state: &AppState,
    endpoint_id: String,
) -> Result<(), ReceiverError> {
    {
        let db = state.db.lock().await;
        db.set_forwarder_intent(&endpoint_id, false)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    state.recompute_aggregate_connection_state().await;
    state.wake_reconcile();
    state.emit_resync();
    Ok(())
}

pub async fn reconnect_forwarder(
    state: &AppState,
    endpoint_id: String,
) -> Result<(), ReceiverError> {
    {
        let db = state.db.lock().await;
        db.set_forwarder_intent(&endpoint_id, true)
            .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    }
    state.recompute_aggregate_connection_state().await;
    state.request_forwarder_reconnect(endpoint_id).await;
    state.emit_resync();
    Ok(())
}

/// Error returned when a remote-config command targets a forwarder that has no
/// live control session, or whose session did not negotiate `CAP_REMOTE_CONFIG`.
fn forwarder_remote_config_unavailable() -> ReceiverError {
    ReceiverError::NotConnected("forwarder not connected or remote config unavailable".to_owned())
}

fn enqueue_config_command(
    tx: &mpsc::Sender<ConfigCommand>,
    command: ConfigCommand,
) -> Result<(), ReceiverError> {
    match tx.try_send(command) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(ReceiverError::UpstreamError(
            "forwarder remote config channel busy".to_owned(),
        )),
        Err(TrySendError::Closed(_)) => Err(forwarder_remote_config_unavailable()),
    }
}

/// Await a remote-config `oneshot` response with a bounded timeout. A dropped
/// sender (control session torn down before replying) and an elapsed timeout
/// both surface as errors so the command never hangs.
async fn await_config_response<T>(rx: oneshot::Receiver<T>) -> Result<T, ReceiverError> {
    match tokio::time::timeout(FORWARDER_CONFIG_TIMEOUT, rx).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => Err(ReceiverError::NotConnected(
            "forwarder control session ended before responding".to_owned(),
        )),
        Err(_) => Err(ReceiverError::UpstreamError(
            "timed out waiting for forwarder config response".to_owned(),
        )),
    }
}

/// Fetch the forwarder's full config document over its live P2P control
/// session (requires a negotiated `CAP_REMOTE_CONFIG` session).
pub async fn get_forwarder_config(
    state: &AppState,
    endpoint_id: String,
) -> Result<ForwarderConfigResponse, ReceiverError> {
    let tx = state
        .forwarder_config_tx(&endpoint_id)
        .ok_or_else(forwarder_remote_config_unavailable)?;
    let (resp_tx, resp_rx) = oneshot::channel();
    enqueue_config_command(&tx, ConfigCommand::Get { resp: resp_tx })?;
    let response = await_config_response(resp_rx).await?;
    Ok(ForwarderConfigResponse {
        config_json: response.config_json,
        restart_needed: response.restart_needed,
    })
}

/// Replace the forwarder's config with `config_json` (the full document, sent
/// verbatim — no merge/patch) over its live P2P control session.
pub async fn set_forwarder_config(
    state: &AppState,
    endpoint_id: String,
    config_json: String,
) -> Result<ForwarderConfigSetResult, ReceiverError> {
    let tx = state
        .forwarder_config_tx(&endpoint_id)
        .ok_or_else(forwarder_remote_config_unavailable)?;
    let (resp_tx, resp_rx) = oneshot::channel();
    enqueue_config_command(
        &tx,
        ConfigCommand::Set {
            config_json,
            resp: resp_tx,
        },
    )?;
    let response = await_config_response(resp_rx).await?;
    Ok(ForwarderConfigSetResult {
        ok: response.ok,
        restart_needed: response.restart_needed,
        error: optional_non_empty(response.error),
    })
}

/// Ask the forwarder to restart over its live P2P control session.
pub async fn restart_forwarder(
    state: &AppState,
    endpoint_id: String,
) -> Result<ForwarderRestartResult, ReceiverError> {
    let tx = state
        .forwarder_config_tx(&endpoint_id)
        .ok_or_else(forwarder_remote_config_unavailable)?;
    let (resp_tx, resp_rx) = oneshot::channel();
    enqueue_config_command(&tx, ConfigCommand::Restart { resp: resp_tx })?;
    let response = await_config_response(resp_rx).await?;
    Ok(ForwarderRestartResult {
        accepted: response.accepted,
        error: optional_non_empty(response.error),
    })
}

fn forwarder_reader_control_unavailable() -> ReceiverError {
    ReceiverError::NotConnected("forwarder not connected or reader control unavailable".to_owned())
}

fn enqueue_reader_command(
    tx: &mpsc::Sender<ReaderCommand>,
    command: ReaderCommand,
) -> Result<(), ReceiverError> {
    match tx.try_send(command) {
        Ok(()) => Ok(()),
        Err(TrySendError::Full(_)) => Err(ReceiverError::UpstreamError(
            "forwarder reader control channel busy".to_owned(),
        )),
        Err(TrySendError::Closed(_)) => Err(forwarder_reader_control_unavailable()),
    }
}

async fn await_reader_response(
    rx: oneshot::Receiver<ReaderControlResponse>,
) -> Result<ReaderControlResponse, ReceiverError> {
    match tokio::time::timeout(FORWARDER_CONFIG_TIMEOUT, rx).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(_)) => Err(ReceiverError::NotConnected(
            "forwarder control session ended before responding".to_owned(),
        )),
        Err(_) => Err(ReceiverError::UpstreamError(
            "timed out waiting for reader control response".to_owned(),
        )),
    }
}

async fn reader_control_command(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
    action: rt_domain::ReaderControlAction,
) -> Result<ReaderControlResult, ReceiverError> {
    let tx = state
        .forwarder_reader_control_tx(&endpoint_id)
        .ok_or_else(forwarder_reader_control_unavailable)?;
    let (resp_tx, resp_rx) = oneshot::channel();
    enqueue_reader_command(
        &tx,
        ReaderCommand::Request {
            stream_id: stream_id.clone(),
            action,
            resp: resp_tx,
        },
    )?;
    let response = await_reader_response(resp_rx).await?;
    let reader_info = match response
        .reader_info_json
        .as_deref()
        .filter(|json| !json.is_empty())
    {
        Some(json) => Some(serde_json::from_str(json).map_err(|error| {
            ReceiverError::UpstreamError(format!(
                "forwarder returned invalid reader_info_json: {error}"
            ))
        })?),
        None => None,
    };
    if response.reader_info_json.is_some() {
        state.store_forwarder_reader_info_sync(
            &endpoint_id,
            ReaderInfo {
                stream_id: if response.stream_id.is_empty() {
                    stream_id.into_bytes()
                } else {
                    response.stream_id.clone()
                },
                hardware_reader_id: String::new(),
                firmware_version: String::new(),
                model: String::new(),
                reader_info_json: response.reader_info_json.clone(),
            },
        );
        state.recompute_aggregate_connection_state().await;
    }
    Ok(ReaderControlResult {
        success: response.success,
        message: response.message,
        reader_info,
    })
}

pub async fn reader_get_info(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::GetInfo,
    )
    .await
}

pub async fn reader_sync_clock(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::SyncClock,
    )
    .await
}

pub async fn reader_set_read_mode(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
    mode: rt_domain::ReadMode,
    timeout: u8,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::SetReadMode { mode, timeout },
    )
    .await
}

pub async fn reader_set_tto(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
    enabled: bool,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::SetTto { enabled },
    )
    .await
}

pub async fn reader_set_recording(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
    enabled: bool,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::SetRecording { enabled },
    )
    .await
}

pub async fn reader_clear_records(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::ClearRecords,
    )
    .await
}

pub async fn reader_start_download(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::StartDownload,
    )
    .await
}

pub async fn reader_stop_download(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::StopDownload,
    )
    .await
}

pub async fn reader_refresh(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::Refresh,
    )
    .await
}

pub async fn reader_reconnect(
    state: &AppState,
    endpoint_id: String,
    stream_id: String,
) -> Result<ReaderControlResult, ReceiverError> {
    reader_control_command(
        state,
        endpoint_id,
        stream_id,
        rt_domain::ReaderControlAction::Reconnect,
    )
    .await
}

pub async fn get_logs(state: &AppState) -> LogsResponse {
    let entries = state.logger.entries();
    LogsResponse { entries }
}

pub fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

pub async fn get_dbf_config(state: &AppState) -> Result<crate::db::DbfConfig, ReceiverError> {
    let db = state.db.lock().await;
    match db.load_dbf_config() {
        Ok(config) => Ok(config),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn put_dbf_config(
    state: &AppState,
    body: crate::db::DbfConfig,
) -> Result<(), ReceiverError> {
    let trimmed = body.path.trim();
    if trimmed.is_empty() {
        return Err(ReceiverError::BadRequest(
            "DBF path must not be empty".to_owned(),
        ));
    }
    let p = std::path::Path::new(trimmed);
    if let Some(ext) = p.extension()
        && !ext.eq_ignore_ascii_case("dbf")
    {
        return Err(ReceiverError::BadRequest(
            "DBF path should have a .dbf extension for Race Director compatibility".to_owned(),
        ));
    }
    if body.enabled
        && let Some(parent) = p.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        return Err(ReceiverError::BadRequest(format!(
            "parent directory does not exist: {}",
            parent.display()
        )));
    }
    let config = crate::db::DbfConfig {
        enabled: body.enabled,
        path: trimmed.to_owned(),
    };
    let db = state.db.lock().await;
    match db.save_dbf_config(&config) {
        Ok(()) => {
            drop(db);
            state.notify_dbf_config_changed();
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn clear_dbf(state: &AppState) -> Result<(), ReceiverError> {
    let db = state.db.lock().await;
    let config = db
        .load_dbf_config()
        .map_err(|e| ReceiverError::Internal(e.to_string()))?;
    drop(db);
    let path = config.path.clone();
    let p = std::path::Path::new(&path);
    if let Some(parent) = p.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        return Err(ReceiverError::BadRequest(format!(
            "DBF directory does not exist: {}",
            parent.display()
        )));
    }
    tokio::task::spawn_blocking(move || crate::dbf_writer::clear_dbf(std::path::Path::new(&path)))
        .await
        .map_err(|e| ReceiverError::Internal(format!("Failed to clear DBF: {e}")))?
        .map_err(|e| ReceiverError::Internal(format!("Failed to clear DBF: {e}")))
}

pub async fn update_subscription_event_type(
    state: &AppState,
    forwarder_endpoint_id: &str,
    stream_id: &str,
    body: EventTypeRequest,
) -> Result<(), ReceiverError> {
    let db = state.db.lock().await;
    match db.update_stream_subscription_event_type(
        forwarder_endpoint_id,
        stream_id,
        body.event_type,
    ) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ReceiverError::BadRequest(
            "subscription not found".to_owned(),
        )),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_reset_cursor(
    state: &AppState,
    body: CursorResetRequest,
) -> Result<(), ReceiverError> {
    let db = state.db.lock().await;
    match db.delete_stream_cursor(&body.stream_id) {
        Ok(()) => Ok(()),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_reset_all_cursors(state: &AppState) -> Result<serde_json::Value, ReceiverError> {
    let db = state.db.lock().await;
    match db.delete_all_cursors() {
        Ok(count) => Ok(serde_json::json!({ "deleted": count })),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_reset_all_earliest_epochs(
    state: &AppState,
) -> Result<serde_json::Value, ReceiverError> {
    let db = state.db.lock().await;
    match db.delete_all_earliest_epochs() {
        Ok(count) => Ok(serde_json::json!({ "deleted": count })),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_reset_earliest_epoch(
    state: &AppState,
    body: CursorResetRequest,
) -> Result<(), ReceiverError> {
    let db = state.db.lock().await;
    match db.delete_stream_earliest_epoch(&body.stream_id) {
        Ok(()) => Ok(()),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_purge_subscriptions(
    state: &AppState,
) -> Result<serde_json::Value, ReceiverError> {
    let db = state.db.lock().await;
    match db.delete_all_subscriptions() {
        Ok(count) => {
            drop(db);
            let conn_for_status = state.connection_state.borrow().clone();
            let db = state.db.lock().await;
            let streams_count = db.load_stream_subscriptions().map(|s| s.len()).unwrap_or(0);
            let receiver_id = state.receiver_id.read().await.clone();
            let _ = state.ui_tx.send(ReceiverUiEvent::StatusChanged {
                connection_state: conn_for_status,
                streams_count,
                receiver_id,
            });
            drop(db);
            state.emit_streams_snapshot().await;
            let conn_for_reconnect = state.connection_state.borrow().clone();
            if matches!(
                conn_for_reconnect,
                ConnectionState::Connected
                    | ConnectionState::Connecting
                    | ConnectionState::Disconnected
            ) {
                state.request_connect().await;
            }
            Ok(serde_json::json!({ "deleted": count }))
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_reset_profile(state: &AppState) -> Result<(), ReceiverError> {
    let current = state.connection_state.borrow().clone();
    if current != ConnectionState::Disconnected {
        state
            .set_connection_state(ConnectionState::Disconnecting)
            .await;
        state.request_disconnect_shutdown();
    }
    let db = state.db.lock().await;
    match db.reset_profile() {
        Ok(()) => {
            drop(db);
            *state.receiver_id.write().await = String::new();
            // The server URL+token were cleared; rebind the always-on P2P
            // runtime so it drops its old server-bound tasks immediately
            // instead of waiting for a later profile save.
            state.notify_server_config_changed();
            state.emit_streams_snapshot().await;
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_clear_data(state: &AppState) -> Result<(), ReceiverError> {
    let current = state.connection_state.borrow().clone();
    if current != ConnectionState::Disconnected {
        state
            .set_connection_state(ConnectionState::Disconnecting)
            .await;
        state.request_disconnect_shutdown();
    }
    let mut db = state.db.lock().await;
    match db.clear_data() {
        Ok(()) => {
            drop(db);
            state.notify_dbf_config_changed();
            state.emit_streams_snapshot().await;
            state.emit_resync();
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_factory_reset(state: &AppState) -> Result<(), ReceiverError> {
    let current = state.connection_state.borrow().clone();
    if current != ConnectionState::Disconnected {
        state
            .set_connection_state(ConnectionState::Disconnecting)
            .await;
        state.request_disconnect_shutdown();
    }
    let mut db = state.db.lock().await;
    match db.factory_reset() {
        Ok(()) => {
            drop(db);
            *state.receiver_id.write().await = String::new();
            // Drop the now-empty participant/chip lookup from memory so a
            // factory reset does not leave prior identities resolvable.
            if let Err(e) = reload_chip_lookup(state).await {
                warn!(error = %e, "failed to reload chip lookup after factory reset");
            }
            // The server config was wiped; rebind the always-on P2P runtime.
            state.notify_server_config_changed();
            state.emit_streams_snapshot().await;
            Ok(())
        }
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

pub async fn admin_update_port(
    state: &AppState,
    body: UpdatePortRequest,
) -> Result<(), ReceiverError> {
    if let Some(0) = body.local_port_override {
        return Err(ReceiverError::BadRequest("port must be 1-65535".to_owned()));
    }
    let db = state.db.lock().await;
    match db.update_stream_subscription_port(
        &body.forwarder_endpoint_id,
        &body.stream_id,
        body.local_port_override,
    ) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ReceiverError::NotFound("subscription not found".to_owned())),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

// ---------------------------------------------------------------------------
// Command & event registry (single source of truth)
//
// This registry enumerates every receiver control command and every UI event
// name in one place. It drives two transports:
//
//   * the Tauri IPC layer (`generate_handler!` in the receiver-tauri app), and
//   * the headless / test bridge mounted in T5.1 (`bridge_command_names`).
//
// A parity test (`tauri_and_bridge_command_sets_match`) asserts both transports
// expose an identical command set, and `event_names_match` asserts the emitted
// event names equal [`EVENT_NAMES`]. Keep all names snake_case and stable.
// ---------------------------------------------------------------------------

/// A single argument of a [`CommandSpec`], excluding the injected app state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandArg {
    /// Argument name as deserialized from the request payload (snake_case).
    pub name: &'static str,
    /// Rust type of the argument, for documentation / bridge codegen.
    pub ty: &'static str,
}

/// Describes one receiver control command: its stable name, its arguments
/// (excluding injected app state), and its success return type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    /// Stable, snake_case command name shared by every transport.
    pub name: &'static str,
    /// Arguments the caller supplies, excluding the injected app state.
    pub args: &'static [CommandArg],
    /// Success return type (the `T` of the command's `Result<T, _>`).
    pub return_type: &'static str,
}

/// Convenience for declaring a [`CommandArg`].
const fn arg(name: &'static str, ty: &'static str) -> CommandArg {
    CommandArg { name, ty }
}

/// Canonical, single-source list of every receiver control command.
///
/// This macro is the one place the full command surface is enumerated, with
/// each command's identifier, argument names/types, and return type. It is
/// `#[macro_export]`ed so the `receiver-tauri` crate can expand the very same
/// list into `tauri::generate_handler!` (and its test-only name list) without
/// maintaining a second copy.
///
/// The macro takes a callback macro path (captured as raw tokens so multi-
/// segment paths can be spliced before `!`) and invokes it with one entry per
/// command in the form `name(arg: "Ty", ...) -> "ReturnType",`. Each consumer
/// supplies an adapter macro that pattern-matches that shape and keeps only
/// the parts it needs (full metadata for [`COMMAND_REGISTRY`], just the
/// identifiers for `generate_handler!`).
#[macro_export]
macro_rules! receiver_command_list {
    ($($callback:tt)+) => {
        $($callback)+! {
            get_profile() -> "ProfileResponse",
            put_profile(body: "ProfileRequest") -> "()",
            get_mode() -> "ReceiverMode",
            put_mode(mode: "ReceiverMode") -> "()",
            get_streams() -> "StreamsResponse",
            get_stream_metrics() -> "Vec<StreamMetricsPayload>",
            put_earliest_epoch(body: "EarliestEpochRequest") -> "()",
            get_replay_target_epochs(
                stream_id: "String"
            ) -> "ReplayTargetEpochsResponse",
            get_subscriptions() -> "SubscriptionsBody",
            put_subscriptions(body: "SubscriptionsBody") -> "()",
            get_status() -> "StatusResponse",
            get_connections() -> "ConnectionsResponse",
            reconnect_server() -> "()",
            connect_forwarder(endpoint_id: "String") -> "()",
            disconnect_forwarder(endpoint_id: "String") -> "()",
            reconnect_forwarder(endpoint_id: "String") -> "()",
            get_forwarder_config(endpoint_id: "String") -> "ForwarderConfigResponse",
            set_forwarder_config(
                endpoint_id: "String",
                config_json: "String"
            ) -> "ForwarderConfigSetResult",
            restart_forwarder(endpoint_id: "String") -> "ForwarderRestartResult",
            reader_get_info(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_sync_clock(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_set_read_mode(
                endpoint_id: "String",
                stream_id: "String",
                mode: "ReadMode",
                timeout: "u8"
            ) -> "ReaderControlResult",
            reader_set_tto(
                endpoint_id: "String",
                stream_id: "String",
                enabled: "bool"
            ) -> "ReaderControlResult",
            reader_set_recording(
                endpoint_id: "String",
                stream_id: "String",
                enabled: "bool"
            ) -> "ReaderControlResult",
            reader_clear_records(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_start_download(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_stop_download(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_refresh(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            reader_reconnect(endpoint_id: "String", stream_id: "String") -> "ReaderControlResult",
            get_version() -> "String",
            get_logs() -> "LogsResponse",
            admin_reset_cursor(body: "CursorResetRequest") -> "()",
            admin_reset_all_cursors() -> "serde_json::Value",
            admin_reset_earliest_epoch(body: "CursorResetRequest") -> "()",
            admin_reset_all_earliest_epochs() -> "serde_json::Value",
            admin_purge_subscriptions() -> "serde_json::Value",
            admin_update_port(body: "UpdatePortRequest") -> "()",
            admin_reset_profile() -> "()",
            admin_clear_data() -> "()",
            admin_factory_reset() -> "()",
            get_dbf_config() -> "DbfConfig",
            put_dbf_config(body: "DbfConfig") -> "()",
            clear_dbf() -> "()",
            update_subscription_event_type(
                forwarder_endpoint_id: "String",
                stream_id: "String",
                body: "EventTypeRequest"
            ) -> "()",
            import_participants(contents: "String") -> "ImportSummary",
            import_chips(contents: "String") -> "ImportSummary",
            import_participants_file(path: "String") -> "ImportSummary",
            import_chips_file(path: "String") -> "ImportSummary",
            get_data_stats() -> "DataStats",
            set_announcer_enabled(enabled: "bool") -> "()",
            set_announcer_max_list_size(max_list_size: "u32") -> "()",
            set_stream_announcer_publish(
                stream_id: "String",
                publish: "bool"
            ) -> "()",
        }
    };
}

/// Adapter that turns one [`receiver_command_list!`] entry into a
/// [`CommandSpec`] and collects them into [`COMMAND_REGISTRY`].
macro_rules! declare_command_registry {
    ($($name:ident ( $($arg:ident : $argty:literal),* $(,)? ) -> $ret:literal),* $(,)?) => {
        /// Canonical registry of every receiver control command.
        ///
        /// Built from [`receiver_command_list!`], the single source of truth
        /// shared with the Tauri `generate_handler!` list. The
        /// `tauri_and_bridge_command_sets_match` parity test asserts the two
        /// transports expose an identical command set.
        pub const COMMAND_REGISTRY: &[CommandSpec] = &[
            $(CommandSpec {
                name: stringify!($name),
                args: &[$(arg(stringify!($arg), $argty)),*],
                return_type: $ret,
            }),*
        ];
    };
}

receiver_command_list!(declare_command_registry);

/// Every command name in the canonical registry, in registry order.
pub fn command_names() -> Vec<&'static str> {
    COMMAND_REGISTRY.iter().map(|c| c.name).collect()
}

/// Command names the headless / test bridge (T5.1) mounts routes from.
///
/// Identical to [`command_names`]; named separately so bridge code reads from
/// an intent-revealing entry point and the parity test compares two
/// independently-derived surfaces rather than a list against itself.
pub fn bridge_command_names() -> Vec<&'static str> {
    command_names()
}

/// Look up a command spec by its stable name.
pub fn command_spec(name: &str) -> Option<&'static CommandSpec> {
    COMMAND_REGISTRY.iter().find(|c| c.name == name)
}

/// Canonical, snake_case names of every UI event the receiver emits.
///
/// These are the event names carried over both the Tauri IPC layer
/// (`ui_event_name`) and the headless / test bridge. [`event_name`] maps a
/// concrete [`ReceiverUiEvent`] to its entry here.
pub const EVENT_NAMES: &[&str] = &[
    "resync",
    "status_changed",
    "connections_changed",
    "streams_snapshot",
    "log_entry",
    "stream_counts_updated",
    "forwarder_metrics_updated",
    "mode_changed",
    "last_read",
    "stream_metrics_updated",
    "forwarder_ups_updated",
];

/// Canonical event name for a [`ReceiverUiEvent`].
///
/// This is the single source of truth for event naming, consumed by both the
/// Tauri bridge and the headless / test bridge. The match is exhaustive so that
/// adding a variant forces a naming decision at compile time.
pub fn event_name(event: &ReceiverUiEvent) -> &'static str {
    match event {
        ReceiverUiEvent::Resync => "resync",
        ReceiverUiEvent::StatusChanged { .. } => "status_changed",
        ReceiverUiEvent::ConnectionsChanged => "connections_changed",
        ReceiverUiEvent::StreamsSnapshot { .. } => "streams_snapshot",
        ReceiverUiEvent::LogEntry { .. } => "log_entry",
        ReceiverUiEvent::StreamCountsUpdated { .. } => "stream_counts_updated",
        ReceiverUiEvent::ForwarderMetricsUpdated(_) => "forwarder_metrics_updated",
        ReceiverUiEvent::ModeChanged { .. } => "mode_changed",
        ReceiverUiEvent::LastRead(_) => "last_read",
        ReceiverUiEvent::StreamMetricsUpdated(_) => "stream_metrics_updated",
        ReceiverUiEvent::ForwarderUpsUpdated { .. } => "forwarder_ups_updated",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, Profile};

    fn profile(url: &str, token: &str) -> Profile {
        Profile {
            server_url: url.to_owned(),
            token: token.to_owned(),
            update_mode: String::new(),
            receiver_id: Some("recv-1".to_owned()),
        }
    }

    #[test]
    fn server_fields_to_persist_preserves_stored_when_env_active() {
        // Env override active: the echoed env values are ignored; the stored
        // (DB) server fields are preserved so the env token never lands in DB.
        let stored = profile("http://stored", "stored-token");
        let (url, token) = server_fields_to_persist(
            true,
            "http://env".to_owned(),
            "env-token".to_owned(),
            Some(&stored),
        );
        assert_eq!(url, "http://stored");
        assert_eq!(token, "stored-token");

        // No stored profile + env active: persist empty, never the env token.
        let (url, token) =
            server_fields_to_persist(true, "http://env".to_owned(), "env-token".to_owned(), None);
        assert_eq!(url, "");
        assert_eq!(token, "");
    }

    #[test]
    fn server_fields_to_persist_uses_body_when_no_env_override() {
        let stored = profile("http://stored", "stored-token");
        let (url, token) = server_fields_to_persist(
            false,
            "http://body".to_owned(),
            "body-token".to_owned(),
            Some(&stored),
        );
        assert_eq!(url, "http://body");
        assert_eq!(token, "body-token");
    }

    async fn count_connections_changed_events(
        ui_rx: &mut tokio::sync::broadcast::Receiver<ReceiverUiEvent>,
    ) -> usize {
        let mut count = 0;
        while let Ok(Ok(event)) =
            tokio::time::timeout(std::time::Duration::from_millis(25), ui_rx.recv()).await
        {
            if matches!(event, ReceiverUiEvent::ConnectionsChanged) {
                count += 1;
            }
        }
        count
    }

    #[test]
    fn derive_forwarder_state_pending_grace_expires_to_unavailable() {
        // Intent to connect, control not up, and the pending clock was started
        // more than the grace window ago: the forwarder must read as
        // Unavailable with `pending=false` (the grace has elapsed).
        let runtime = ForwarderRuntimeStatus {
            control_up: false,
            data_sessions: 0,
            pending_started_at: Some(std::time::Instant::now() - std::time::Duration::from_secs(6)),
        };
        let snapshot = derive_forwarder_state(runtime, true);
        assert_eq!(snapshot.state, ForwarderConnState::Unavailable);
        assert!(
            !snapshot.pending,
            "grace older than FORWARDER_PENDING_GRACE must clear pending"
        );
    }

    #[test]
    fn derive_forwarder_state_within_grace_stays_pending() {
        // Same as above but the pending clock just started: still within the
        // grace window, so `pending=true` (not yet treated as Unavailable to
        // the user, even though the underlying state is Unavailable).
        let runtime = ForwarderRuntimeStatus {
            control_up: false,
            data_sessions: 0,
            pending_started_at: Some(std::time::Instant::now()),
        };
        let snapshot = derive_forwarder_state(runtime, true);
        assert_eq!(snapshot.state, ForwarderConnState::Unavailable);
        assert!(
            snapshot.pending,
            "a freshly started grace clock must keep pending=true"
        );
    }

    #[tokio::test]
    async fn import_participants_and_chips_populate_lookup() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let p = import_participants(&state, ";hdr\n1,Smith,John,,,M\n2,Doe,Jane\n".to_owned())
            .await
            .unwrap();
        assert_eq!(p.imported, 2);
        let c = import_chips(&state, "BIB,CHIP\n1,0580\n2,0581\n".to_owned())
            .await
            .unwrap();
        assert_eq!(c.imported, 2);
        assert_eq!(c.resolvable_chips, 2);
        let stats = get_data_stats(&state).await.unwrap();
        assert_eq!(stats.participants, 2);
        assert_eq!(stats.chips, 2);
        assert_eq!(stats.matched_participants, 2);
        assert_eq!(stats.participants_without_chips, 0);
        assert_eq!(stats.resolvable_chips, 2);
        let lookup = state.chip_lookup.read().await;
        let resolved = lookup
            .values()
            .find_map(|chips| chips.get("0580"))
            .expect("chip resolves");
        assert_eq!(resolved, &("1".to_owned(), "John Smith".to_owned()));
    }

    #[tokio::test]
    async fn import_participants_file_reads_selected_path() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, b"1,Smith,John\n").unwrap();

        let summary = import_participants_file(&state, file.path().to_string_lossy().into_owned())
            .await
            .unwrap();

        assert_eq!(summary.imported, 1);
        let stats = get_data_stats(&state).await.unwrap();
        assert_eq!(stats.participants, 1);
    }

    #[tokio::test]
    async fn import_participants_file_falls_back_to_windows_1252() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, b"1,Dupont,Ren\xE9\n").unwrap();

        import_participants_file(&state, file.path().to_string_lossy().into_owned())
            .await
            .unwrap();
        import_chips(&state, "1,aaaa\n".to_owned()).await.unwrap();

        let lookup = state.chip_lookup.read().await;
        let resolved = lookup
            .values()
            .find_map(|chips| chips.get("aaaa"))
            .expect("chip resolves");
        assert_eq!(resolved, &("1".to_owned(), "René Dupont".to_owned()));
    }

    #[tokio::test]
    async fn data_stats_reports_participants_missing_chips() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        import_participants(&state, "1,Smith,John\n2,Doe,Jane\n3,Roe,Rich\n".to_owned())
            .await
            .unwrap();
        import_chips(&state, "1,aaa\n2,bbb\n99,deadbeef\n".to_owned())
            .await
            .unwrap();

        let stats = get_data_stats(&state).await.unwrap();
        assert_eq!(stats.participants, 3);
        assert_eq!(stats.chips, 3);
        assert_eq!(stats.matched_participants, 2);
        assert_eq!(stats.participants_without_chips, 1);
        assert_eq!(stats.resolvable_chips, 2);
    }

    #[tokio::test]
    async fn malformed_import_is_rejected_without_mutation() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        // Seed a good participant set first.
        import_participants(&state, "1,Smith,John\n".to_owned())
            .await
            .unwrap();
        // A malformed row must reject the whole upload and not touch the table.
        let err = import_participants(&state, "2,Good,Row\nz,bad,bib\n".to_owned())
            .await
            .unwrap_err();
        assert!(matches!(err, ReceiverError::BadRequest(_)));
        // Verify bib 2 was never written: chips for bib 1 and bib 2 should leave
        // only bib 1 resolvable.
        let summary = import_chips(&state, "1,aaa\n2,bbb\n".to_owned())
            .await
            .unwrap();
        assert_eq!(summary.imported, 2);
        assert_eq!(
            summary.resolvable_chips, 1,
            "only bib 1 should resolve; the rejected import must not have added bib 2"
        );
        let stats = get_data_stats(&state).await.unwrap();
        assert_eq!(stats.participants, 1);
        assert_eq!(stats.chips, 2);
        assert_eq!(stats.matched_participants, 1);
        assert_eq!(stats.participants_without_chips, 0);
        assert_eq!(stats.resolvable_chips, 1);
    }

    #[tokio::test]
    async fn reload_chip_lookup_populates_resolver() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        {
            let mut db = state.db.lock().await;
            db.replace_participants(&[crate::participants::Participant {
                bib: 12,
                last: "Runner".to_owned(),
                first: "Fast".to_owned(),
                affiliation: String::new(),
                gender: "X".to_owned(),
            }])
            .unwrap();
            db.replace_bib_chips(&[(12, "chip-12".to_owned())]).unwrap();
        }
        let count = reload_chip_lookup(&state).await.unwrap();
        assert_eq!(count, 1);
        let lookup = state.chip_lookup.read().await;
        let resolved = lookup
            .values()
            .find_map(|chips| chips.get("chip-12"))
            .expect("chip resolves");
        assert_eq!(resolved, &("12".to_owned(), "Fast Runner".to_owned()));
    }

    #[tokio::test]
    async fn server_config_version_signal_is_observable() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let mut rx = state.server_config_rx();
        state.notify_server_config_changed();
        assert!(rx.changed().await.is_ok());
    }

    #[tokio::test]
    async fn put_profile_signals_server_config_change() {
        let (state, _rx) = AppState::new(Db::open_in_memory().unwrap(), "recv".to_owned());
        let mut rx = state.server_config_rx();
        put_profile(
            &state,
            ProfileRequest {
                server_url: "http://127.0.0.1:8080".to_owned(),
                token: "tok".to_owned(),
                receiver_id: None,
            },
        )
        .await
        .expect("put_profile ok");
        assert!(rx.changed().await.is_ok());
    }

    #[test]
    fn command_registry_names_are_unique() {
        let mut names: Vec<&str> = command_names();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(
            names.len(),
            count,
            "COMMAND_REGISTRY contains duplicate names"
        );
    }

    #[test]
    fn command_registry_names_are_snake_case() {
        for spec in COMMAND_REGISTRY {
            assert!(
                spec.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "command name {:?} is not snake_case",
                spec.name
            );
        }
    }

    #[test]
    fn command_specs_have_return_type_and_unique_arg_names() {
        for spec in COMMAND_REGISTRY {
            assert!(
                !spec.return_type.is_empty(),
                "command {:?} has an empty return_type",
                spec.name
            );
            let mut arg_names: Vec<&str> = spec.args.iter().map(|a| a.name).collect();
            let total = arg_names.len();
            arg_names.sort_unstable();
            arg_names.dedup();
            assert_eq!(
                arg_names.len(),
                total,
                "command {:?} has duplicate argument names",
                spec.name
            );
            for a in spec.args {
                assert!(
                    !a.name.is_empty(),
                    "command {:?} has an empty argument name",
                    spec.name
                );
                assert!(
                    !a.ty.is_empty(),
                    "command {:?} arg {:?} has an empty type",
                    spec.name,
                    a.name
                );
            }
        }
    }

    #[test]
    fn command_spec_lookup_round_trips() {
        for spec in COMMAND_REGISTRY {
            assert_eq!(command_spec(spec.name), Some(spec));
        }
        assert_eq!(command_spec("does_not_exist"), None);
    }

    #[test]
    fn command_registry_uses_stream_identity_args_for_subscription_updates() {
        let spec = command_spec("update_subscription_event_type").unwrap();
        let arg_names: Vec<&str> = spec.args.iter().map(|arg| arg.name).collect();
        assert_eq!(
            arg_names,
            vec!["forwarder_endpoint_id", "stream_id", "body"]
        );
        assert!(!arg_names.contains(&"reader_ip"));
    }

    #[test]
    fn event_name_maps_into_canonical_event_names() {
        // Every variant's event_name must be present in EVENT_NAMES; the match
        // in `event_name` is exhaustive so adding a variant forces a decision.
        let samples = [
            ReceiverUiEvent::Resync,
            ReceiverUiEvent::LogEntry {
                entry: String::new(),
            },
        ];
        for event in &samples {
            assert!(
                EVENT_NAMES.contains(&event_name(event)),
                "event_name produced {:?} which is missing from EVENT_NAMES",
                event_name(event)
            );
        }
    }

    #[test]
    fn event_names_are_unique_and_snake_case() {
        let mut names = EVENT_NAMES.to_vec();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "EVENT_NAMES contains duplicates");
        for name in EVENT_NAMES {
            assert!(
                name.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
                "event name {name:?} is not snake_case"
            );
        }
    }

    #[test]
    fn connections_changed_maps_to_expected_event_name() {
        assert_eq!(
            event_name(&ReceiverUiEvent::ConnectionsChanged),
            "connections_changed"
        );
    }

    #[tokio::test]
    async fn get_connections_reports_forwarder_reader_and_ups_live_status() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-a".to_owned(),
            stream_id: "stream-a".to_owned(),
            local_port_override: Some(9100),
            event_type: crate::db::EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.discovered_forwarders.write().await.insert(
            "endpoint-a".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Finish Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: Vec::new(),
            },
        );

        state
            .record_forwarder_reader_status(
                "endpoint-a",
                rt_p2p_protocol::ReaderStatus {
                    stream_id: b"stream-b".to_vec(),
                    connected: true,
                    state: "online".to_owned(),
                    last_read_unix_ms: 1234,
                },
            )
            .await;
        state
            .record_forwarder_reader_status(
                "endpoint-a",
                rt_p2p_protocol::ReaderStatus {
                    stream_id: b"stream-a".to_vec(),
                    connected: false,
                    state: "offline".to_owned(),
                    last_read_unix_ms: 0,
                },
            )
            .await;
        let rich_reader_info = rt_domain::ReaderInfo {
            hardware: Some(rt_domain::HardwareInfo {
                fw_version: Some("1.2.3".to_owned()),
                hw_code: Some("IPICO".to_owned()),
                reader_id: Some("reader-42".to_owned()),
            }),
            config: Some(rt_domain::Config3Info {
                mode: rt_domain::ReadMode::Event,
                timeout: 7,
            }),
            tto_enabled: Some(true),
            ..rt_domain::ReaderInfo {
                banner: None,
                hardware: None,
                config: None,
                tto_enabled: None,
                clock: None,
                estimated_stored_reads: None,
                recording: None,
                connect_failures: 0,
            }
        };
        state
            .record_forwarder_reader_info(
                "endpoint-a",
                rt_p2p_protocol::ReaderInfo {
                    stream_id: b"stream-a".to_vec(),
                    hardware_reader_id: "reader-42".to_owned(),
                    firmware_version: "1.2.3".to_owned(),
                    model: "IPICO".to_owned(),
                    reader_info_json: Some(serde_json::to_string(&rich_reader_info).unwrap()),
                },
            )
            .await;
        state
            .record_forwarder_download_progress(
                "endpoint-a",
                rt_p2p_protocol::DownloadProgress {
                    stream_id: b"stream-a".to_vec(),
                    downloaded_bytes: 0,
                    total_bytes: 0,
                    state: "downloading".to_owned(),
                    reads_received: 42,
                    progress: 42,
                    total: 100,
                    error: String::new(),
                },
            )
            .await;
        state
            .record_forwarder_reader_status(
                "endpoint-a",
                rt_p2p_protocol::ReaderStatus {
                    stream_id: b"stream-a".to_vec(),
                    connected: true,
                    state: "online".to_owned(),
                    last_read_unix_ms: 5678,
                },
            )
            .await;
        state
            .record_forwarder_ups_status(
                "endpoint-a",
                rt_p2p_protocol::UpsStatus {
                    on_battery: true,
                    battery_percent: 87,
                    runtime_seconds: 1200,
                },
            )
            .await;

        let response = get_connections(&state).await;

        let forwarder = response
            .forwarders
            .iter()
            .find(|forwarder| forwarder.endpoint_id == "endpoint-a")
            .expect("forwarder should be present");
        assert_eq!(forwarder.readers.len(), 2);
        assert_eq!(forwarder.readers[0].stream_id, "stream-a");
        assert!(forwarder.readers[0].connected);
        assert_eq!(forwarder.readers[0].state, "online");
        assert_eq!(forwarder.readers[0].last_read_unix_ms, Some(5678));
        assert_eq!(forwarder.readers[0].local_port, Some(9100));
        assert_eq!(
            forwarder.readers[0].hardware_reader_id.as_deref(),
            Some("reader-42")
        );
        assert_eq!(
            forwarder.readers[0].firmware_version.as_deref(),
            Some("1.2.3")
        );
        assert_eq!(forwarder.readers[0].model.as_deref(), Some("IPICO"));
        assert_eq!(forwarder.readers[0].reader_info, Some(rich_reader_info));
        assert_eq!(
            forwarder.readers[0].download_progress,
            Some(rt_domain::DownloadProgressUpdate {
                reader_ip: "stream-a".to_owned(),
                state: rt_domain::DownloadState::Downloading,
                stored_reads: Some(100),
                downloaded_reads: 42,
                progress: 42,
                total: Some(100),
                last_read_at: None,
                error: None,
            })
        );
        assert_eq!(forwarder.readers[1].stream_id, "stream-b");
        assert!(forwarder.readers[1].connected);
        assert_eq!(forwarder.readers[1].last_read_unix_ms, Some(1234));
        assert_eq!(forwarder.readers[1].local_port, None);
        let ups = forwarder
            .ups
            .as_ref()
            .expect("UPS status should be present");
        assert!(ups.on_battery);
        assert_eq!(ups.battery_percent, 87);
        assert_eq!(ups.runtime_seconds, 1200);
    }

    #[tokio::test]
    async fn clearing_forwarder_live_status_removes_stale_reader_status() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.discovered_forwarders.write().await.insert(
            "endpoint-a".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Finish Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: Vec::new(),
            },
        );
        state
            .record_forwarder_reader_status(
                "endpoint-a",
                rt_p2p_protocol::ReaderStatus {
                    stream_id: b"stream-a".to_vec(),
                    connected: true,
                    state: "online".to_owned(),
                    last_read_unix_ms: 10,
                },
            )
            .await;

        state.clear_forwarder_live_status("endpoint-a").await;

        let response = get_connections(&state).await;
        let forwarder = response
            .forwarders
            .iter()
            .find(|forwarder| forwarder.endpoint_id == "endpoint-a")
            .expect("forwarder should be present");
        assert!(forwarder.readers.is_empty());
        assert!(forwarder.ups.is_none());
    }

    #[tokio::test]
    async fn live_status_updates_emit_connections_changed_only_on_actual_change() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.discovered_forwarders.write().await.insert(
            "endpoint-a".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Finish Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: Vec::new(),
            },
        );
        state.recompute_aggregate_connection_state().await;
        let mut ui_rx = state.ui_tx.subscribe();

        state
            .record_forwarder_ups_status(
                "endpoint-a",
                rt_p2p_protocol::UpsStatus {
                    on_battery: false,
                    battery_percent: 95,
                    runtime_seconds: 3600,
                },
            )
            .await;
        state
            .record_forwarder_ups_status(
                "endpoint-a",
                rt_p2p_protocol::UpsStatus {
                    on_battery: false,
                    battery_percent: 95,
                    runtime_seconds: 3600,
                },
            )
            .await;

        let connections_changed_count = count_connections_changed_events(&mut ui_rx).await;
        assert_eq!(
            connections_changed_count, 1,
            "unchanged live status should not keep emitting connections_changed"
        );
    }

    #[tokio::test]
    async fn get_connections_returns_sorted_discovered_forwarder_statuses() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-a".to_owned(),
            stream_id: "stream-a".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.discovered_forwarders.write().await.extend([
            (
                "endpoint-b".to_owned(),
                DiscoveredForwarder {
                    display_name: Some("Finish Line".to_owned()),
                    direct_addrs: Vec::new(),
                    streams: Vec::new(),
                },
            ),
            (
                "endpoint-a".to_owned(),
                DiscoveredForwarder {
                    display_name: Some("Start Line".to_owned()),
                    direct_addrs: Vec::new(),
                    streams: vec![
                        DiscoveredStream {
                            stream_id: "stream-a".to_owned(),
                            epoch: 1,
                            next_seq: 10,
                        },
                        DiscoveredStream {
                            stream_id: "stream-b".to_owned(),
                            epoch: 2,
                            next_seq: 20,
                        },
                    ],
                },
            ),
        ]);
        state
            .mark_forwarder_runtime("endpoint-a", |status| status.data_sessions = 1)
            .await;

        let response = get_connections(&state).await;

        assert!(!response.server.configured);
        assert_eq!(response.forwarders.len(), 2);
        assert_eq!(response.forwarders[0].endpoint_id, "endpoint-a");
        assert_eq!(
            response.forwarders[0].display_name.as_deref(),
            Some("Start Line")
        );
        assert_eq!(response.forwarders[0].state, ForwarderConnState::Subscribed);
        assert!(!response.forwarders[0].pending);
        assert_eq!(response.forwarders[0].subscribed_count, 1);
        assert_eq!(response.forwarders[0].available_count, 2);
        assert!(response.forwarders[0].readers.is_empty());
        assert!(response.forwarders[0].ups.is_none());
        assert_eq!(response.forwarders[0].restart_needed, None);
        assert_eq!(response.forwarders[1].endpoint_id, "endpoint-b");
        assert_eq!(
            response.forwarders[1].display_name.as_deref(),
            Some("Finish Line")
        );
        assert_eq!(
            response.forwarders[1].state,
            ForwarderConnState::Unavailable
        );
        assert!(!response.forwarders[1].pending);
        assert_eq!(response.forwarders[1].subscribed_count, 0);
        assert_eq!(response.forwarders[1].available_count, 0);
    }

    #[tokio::test]
    async fn recompute_emits_connections_changed_when_forwarder_state_changes() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut ui_rx = state.ui_tx.subscribe();

        state
            .mark_forwarder_runtime("endpoint-a", |status| status.control_up = true)
            .await;

        let mut saw_connections_changed = false;
        for _ in 0..4 {
            let event = tokio::time::timeout(std::time::Duration::from_millis(100), ui_rx.recv())
                .await
                .expect("forwarder state recompute should emit UI events")
                .expect("UI event channel should stay open");
            if matches!(event, ReceiverUiEvent::ConnectionsChanged) {
                saw_connections_changed = true;
                break;
            }
        }
        assert!(
            saw_connections_changed,
            "forwarder state change should emit ConnectionsChanged"
        );
    }

    #[tokio::test]
    async fn recompute_emits_connections_changed_at_most_once_when_view_is_unchanged() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.discovered_forwarders.write().await.insert(
            "endpoint-a".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Finish Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: vec![DiscoveredStream {
                    stream_id: "stream-a".to_owned(),
                    epoch: 1,
                    next_seq: 10,
                }],
            },
        );
        let mut ui_rx = state.ui_tx.subscribe();

        state.recompute_aggregate_connection_state().await;
        state.recompute_aggregate_connection_state().await;

        let connections_changed_count = count_connections_changed_events(&mut ui_rx).await;
        assert!(
            connections_changed_count <= 1,
            "unchanged connections view should emit at most one ConnectionsChanged event, got {connections_changed_count}"
        );
    }

    #[tokio::test]
    async fn recompute_emits_connections_changed_when_remote_config_availability_changes() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state
            .mark_forwarder_runtime("endpoint-a", |status| status.control_up = true)
            .await;
        let mut ui_rx = state.ui_tx.subscribe();
        let _ = count_connections_changed_events(&mut ui_rx).await;

        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let _guard = state.register_forwarder_config_tx("endpoint-a", tx);
        state.recompute_aggregate_connection_state().await;

        let connections_changed_count = count_connections_changed_events(&mut ui_rx).await;
        assert_eq!(
            connections_changed_count, 1,
            "remote_config_available changing should emit ConnectionsChanged"
        );
        let response = get_connections(&state).await;
        let forwarder = response
            .forwarders
            .iter()
            .find(|forwarder| forwarder.endpoint_id == "endpoint-a")
            .expect("forwarder should be present");
        assert!(forwarder.remote_config_available);
    }

    #[tokio::test]
    async fn config_command_fast_fails_when_session_queue_is_full() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let (tx, _rx) = tokio::sync::mpsc::channel(1);
        let (held_resp_tx, _held_resp_rx) = tokio::sync::oneshot::channel();
        tx.try_send(ConfigCommand::Get { resp: held_resp_tx })
            .expect("test setup should fill the one-slot queue");
        let _guard = state.register_forwarder_config_tx("endpoint-a", tx);

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            get_forwarder_config(&state, "endpoint-a".to_owned()),
        )
        .await;

        assert!(
            result.is_ok(),
            "full config command queue should fail fast, not wait for enqueue capacity"
        );
        assert!(result.unwrap().is_err());
    }

    #[tokio::test]
    async fn reader_control_command_routes_request_and_stores_reader_info() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let _guard = state.register_forwarder_reader_control_tx("endpoint-a", tx);
        let expected_info = rt_domain::ReaderInfo {
            clock: Some(rt_domain::ClockInfo {
                reader_clock: "2026-06-22T12:00:00Z".to_owned(),
                drift_ms: 12,
            }),
            tto_enabled: Some(true),
            ..rt_domain::ReaderInfo {
                banner: None,
                hardware: None,
                config: None,
                tto_enabled: None,
                clock: None,
                estimated_stored_reads: None,
                recording: None,
                connect_failures: 0,
            }
        };
        state
            .record_forwarder_reader_info(
                "endpoint-a",
                rt_p2p_protocol::ReaderInfo {
                    stream_id: b"stream-a".to_vec(),
                    hardware_reader_id: "reader-42".to_owned(),
                    firmware_version: "1.2.3".to_owned(),
                    model: "IPICO".to_owned(),
                    reader_info_json: None,
                },
            )
            .await;

        let response_info = expected_info.clone();
        tokio::spawn(async move {
            let ReaderCommand::Request {
                stream_id,
                action,
                resp,
            } = rx.recv().await.expect("reader command");
            assert_eq!(stream_id, "stream-a");
            assert_eq!(action, rt_domain::ReaderControlAction::SyncClock);
            resp.send(ReaderControlResponse {
                stream_id: b"stream-a".to_vec(),
                request_id: "1".to_owned(),
                success: true,
                message: String::new(),
                reader_info_json: Some(serde_json::to_string(&response_info).unwrap()),
            })
            .expect("send reader response");
        });

        let result = reader_sync_clock(&state, "endpoint-a".to_owned(), "stream-a".to_owned())
            .await
            .expect("reader sync clock");

        assert!(result.success);
        assert_eq!(result.reader_info, Some(expected_info.clone()));
        let response = get_connections(&state).await;
        let reader = response
            .forwarders
            .iter()
            .find(|forwarder| forwarder.endpoint_id == "endpoint-a")
            .and_then(|forwarder| forwarder.readers.first())
            .expect("reader status should be populated from response reader_info_json");
        assert_eq!(reader.reader_info, Some(expected_info));
        assert_eq!(reader.hardware_reader_id.as_deref(), Some("reader-42"));
        assert_eq!(reader.firmware_version.as_deref(), Some("1.2.3"));
        assert_eq!(reader.model.as_deref(), Some("IPICO"));
    }

    #[tokio::test]
    async fn reader_control_command_rejects_invalid_reader_info_json() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let _guard = state.register_forwarder_reader_control_tx("endpoint-a", tx);
        tokio::spawn(async move {
            let ReaderCommand::Request { resp, .. } = rx.recv().await.expect("reader command");
            resp.send(ReaderControlResponse {
                stream_id: b"stream-a".to_vec(),
                request_id: "1".to_owned(),
                success: true,
                message: String::new(),
                reader_info_json: Some("{".to_owned()),
            })
            .expect("send reader response");
        });

        let result = reader_refresh(&state, "endpoint-a".to_owned(), "stream-a".to_owned()).await;

        assert!(matches!(result, Err(ReceiverError::UpstreamError(_))));
    }

    #[tokio::test]
    async fn recompute_emits_connections_changed_again_when_view_changes() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.discovered_forwarders.write().await.insert(
            "endpoint-a".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Finish Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: Vec::new(),
            },
        );
        let mut ui_rx = state.ui_tx.subscribe();

        state.recompute_aggregate_connection_state().await;
        let _ = count_connections_changed_events(&mut ui_rx).await;

        state.update_forwarder_runtime_sync("endpoint-a", |status| status.control_up = true);
        state.recompute_aggregate_connection_state().await;

        let connections_changed_count = count_connections_changed_events(&mut ui_rx).await;
        assert_eq!(
            connections_changed_count, 1,
            "changed connections view should emit another ConnectionsChanged event"
        );
    }

    #[tokio::test]
    async fn build_streams_response_uses_discovered_epoch_for_subscription() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "stream-a".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.discovered_forwarders.write().await.insert(
            "endpoint-1".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Start Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: vec![DiscoveredStream {
                    stream_id: "stream-a".to_owned(),
                    epoch: 7,
                    next_seq: 42,
                }],
            },
        );

        let response = state.build_streams_response().await;

        assert_eq!(response.streams.len(), 1);
        assert!(response.streams[0].subscribed);
        assert_eq!(response.streams[0].stream_epoch, Some(7));
        assert_eq!(
            response.streams[0].display_alias.as_deref(),
            Some("Start Line")
        );
    }

    #[tokio::test]
    async fn build_streams_response_marks_live_subscribed_stream_online() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "stream-a".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: Some("fwd-1".to_owned()),
            reader_ip: Some("10.0.0.1:10000".to_owned()),
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        state.discovered_forwarders.write().await.insert(
            "endpoint-1".to_owned(),
            DiscoveredForwarder {
                display_name: Some("Start Line".to_owned()),
                direct_addrs: Vec::new(),
                streams: vec![DiscoveredStream {
                    stream_id: "stream-a".to_owned(),
                    epoch: 7,
                    next_seq: 42,
                }],
            },
        );
        state
            .mark_forwarder_runtime("endpoint-1", |status| {
                status.control_up = true;
                status.data_sessions = 1;
            })
            .await;
        state
            .record_forwarder_reader_status(
                "endpoint-1",
                rt_p2p_protocol::ReaderStatus {
                    stream_id: b"stream-a".to_vec(),
                    connected: true,
                    state: "connected".to_owned(),
                    last_read_unix_ms: 1234,
                },
            )
            .await;

        let response = state.build_streams_response().await;

        assert_eq!(response.streams.len(), 1);
        assert_eq!(response.streams[0].online, Some(true));
        assert_eq!(response.streams[0].reader_connected, Some(true));
    }

    #[tokio::test]
    async fn reconnect_server_notifies_connect_watchers() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut connect_rx = state.connect_attempt_rx();

        reconnect_server(&state).await.unwrap();

        connect_rx.changed().await.unwrap();
        assert_eq!(connect_rx.borrow().version, 1);
        assert_eq!(connect_rx.borrow().endpoint_id, None);
        assert!(connect_rx.borrow().restart);
        assert_eq!(state.current_connect_attempt(), 1);
        assert_eq!(
            state.connection_state.borrow().clone(),
            ConnectionState::Connecting
        );
    }

    #[tokio::test]
    async fn targeted_forwarder_reconnect_notifies_connect_watchers() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut connect_rx = state.connect_attempt_rx();

        state.request_forwarder_reconnect("fwd-1".to_owned()).await;

        connect_rx.changed().await.unwrap();
        assert_eq!(connect_rx.borrow().version, 1);
        assert_eq!(connect_rx.borrow().endpoint_id.as_deref(), Some("fwd-1"));
        assert!(connect_rx.borrow().restart);
        assert_eq!(state.current_connect_attempt(), 1);
    }

    #[tokio::test]
    async fn disconnect_forwarder_sets_intent_false_preserves_subscriptions_and_notifies_ui() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "stream-1".to_owned(),
            local_port_override: Some(9100),
            event_type: crate::db::EventType::Finish,
            forwarder_id: Some("legacy-fwd".to_owned()),
            reader_ip: Some("10.0.0.1:10000".to_owned()),
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut ui_rx = state.ui_tx.subscribe();

        disconnect_forwarder(&state, "endpoint-1".to_owned())
            .await
            .unwrap();

        {
            let db = state.db.lock().await;
            assert!(!db.forwarder_should_connect("endpoint-1").unwrap());
            assert_eq!(
                db.load_stream_subscriptions().unwrap(),
                vec![crate::db::StreamSubscription {
                    forwarder_endpoint_id: "endpoint-1".to_owned(),
                    stream_id: "stream-1".to_owned(),
                    local_port_override: Some(9100),
                    event_type: crate::db::EventType::Finish,
                    forwarder_id: Some("legacy-fwd".to_owned()),
                    reader_ip: Some("10.0.0.1:10000".to_owned()),
                }]
            );
        }

        let mut saw_connections_changed = false;
        for _ in 0..4 {
            let event = tokio::time::timeout(std::time::Duration::from_millis(100), ui_rx.recv())
                .await
                .expect("disconnect forwarder should emit UI events")
                .expect("UI event channel should stay open");
            if matches!(event, ReceiverUiEvent::ConnectionsChanged) {
                saw_connections_changed = true;
                break;
            }
        }
        assert!(
            saw_connections_changed,
            "disconnect_forwarder should emit ConnectionsChanged"
        );
    }

    #[tokio::test]
    async fn connect_forwarder_sets_intent_true_and_wakes_reconcile() {
        let db = Db::open_in_memory().unwrap();
        db.set_forwarder_intent("endpoint-1", false).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut connect_rx = state.connect_attempt_rx();

        connect_forwarder(&state, "endpoint-1".to_owned())
            .await
            .unwrap();

        {
            let db = state.db.lock().await;
            assert!(db.forwarder_should_connect("endpoint-1").unwrap());
        }
        connect_rx.changed().await.unwrap();
        assert_eq!(connect_rx.borrow().version, 1);
        assert_eq!(connect_rx.borrow().endpoint_id, None);
        assert!(!connect_rx.borrow().restart);
    }

    #[tokio::test]
    async fn reconnect_forwarder_sets_intent_true_and_triggers_targeted_reconnect() {
        let db = Db::open_in_memory().unwrap();
        db.set_forwarder_intent("endpoint-1", false).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut connect_rx = state.connect_attempt_rx();

        reconnect_forwarder(&state, "endpoint-1".to_owned())
            .await
            .unwrap();

        {
            let db = state.db.lock().await;
            assert!(db.forwarder_should_connect("endpoint-1").unwrap());
        }
        connect_rx.changed().await.unwrap();
        assert_eq!(connect_rx.borrow().version, 1);
        assert_eq!(
            connect_rx.borrow().endpoint_id.as_deref(),
            Some("endpoint-1")
        );
        assert!(connect_rx.borrow().restart);
    }

    #[tokio::test]
    async fn put_subscriptions_uses_stream_identity() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        put_subscriptions(
            &state,
            SubscriptionsBody {
                subscriptions: vec![SubscriptionRequest {
                    forwarder_endpoint_id: "endpoint-1".to_owned(),
                    stream_id: "11111111-1111-1111-1111-111111111111".to_owned(),
                    local_port_override: Some(9100),
                    event_type: Some(crate::db::EventType::Start),
                    forwarder_id: None,
                    reader_ip: None,
                }],
            },
        )
        .await
        .unwrap();

        let db = state.db.lock().await;
        let subs = db.load_stream_subscriptions().unwrap();
        assert_eq!(subs.len(), 1);
        assert_eq!(subs[0].forwarder_endpoint_id, "endpoint-1");
        assert_eq!(subs[0].stream_id, "11111111-1111-1111-1111-111111111111");
        assert_eq!(subs[0].forwarder_id, None);
        assert_eq!(subs[0].reader_ip, None);
        assert_eq!(subs[0].local_port_override, Some(9100));
        assert_eq!(subs[0].event_type, crate::db::EventType::Start);
    }

    #[tokio::test]
    async fn admin_reset_cursor_uses_stream_id() {
        let stream_id = "127.0.0.1:10000";
        let db = Db::open_in_memory().unwrap();
        db.jump_stream_cursor(stream_id, 42).unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        admin_reset_cursor(
            &state,
            CursorResetRequest {
                stream_id: stream_id.to_owned(),
            },
        )
        .await
        .unwrap();

        let db = state.db.lock().await;
        assert_eq!(db.load_stream_cursor(stream_id).unwrap(), 0);
    }

    #[tokio::test]
    async fn earliest_epoch_uses_stream_identity() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        put_earliest_epoch(
            &state,
            EarliestEpochRequest {
                forwarder_endpoint_id: "endpoint-1".to_owned(),
                stream_id: "22222222-2222-2222-2222-222222222222".to_owned(),
                earliest_epoch: 7,
            },
        )
        .await
        .unwrap();

        {
            let db = state.db.lock().await;
            // Canonical row is keyed by stream_id with the forwarder endpoint id.
            assert_eq!(
                db.load_stream_earliest_epochs().unwrap(),
                vec![crate::db::StreamEarliestEpoch {
                    stream_id: "22222222-2222-2222-2222-222222222222".to_owned(),
                    forwarder_endpoint_id: "endpoint-1".to_owned(),
                    earliest_epoch: 7,
                }]
            );
            // The legacy (forwarder_id, reader_ip) view must NOT surface the
            // canonical row: stream_id must never be persisted as reader_ip,
            // nor forwarder_endpoint_id as forwarder_id.
            assert!(db.load_earliest_epochs().unwrap().is_empty());
        }

        admin_reset_earliest_epoch(
            &state,
            CursorResetRequest {
                stream_id: "22222222-2222-2222-2222-222222222222".to_owned(),
            },
        )
        .await
        .unwrap();

        let db = state.db.lock().await;
        assert!(db.load_stream_earliest_epochs().unwrap().is_empty());
        assert!(db.load_earliest_epochs().unwrap().is_empty());
    }

    #[tokio::test]
    async fn admin_update_port_uses_stream_identity() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "33333333-3333-3333-3333-333333333333".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: Some("legacy-fwd".to_owned()),
            reader_ip: Some("10.0.0.1:10000".to_owned()),
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        admin_update_port(
            &state,
            UpdatePortRequest {
                forwarder_endpoint_id: "endpoint-1".to_owned(),
                stream_id: "33333333-3333-3333-3333-333333333333".to_owned(),
                local_port_override: Some(9200),
            },
        )
        .await
        .unwrap();

        let db = state.db.lock().await;
        let subs = db.load_stream_subscriptions().unwrap();
        assert_eq!(subs[0].local_port_override, Some(9200));
    }

    #[tokio::test]
    async fn update_subscription_event_type_uses_stream_identity() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "44444444-4444-4444-4444-444444444444".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: Some("legacy-fwd".to_owned()),
            reader_ip: Some("10.0.0.1:10000".to_owned()),
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        update_subscription_event_type(
            &state,
            "endpoint-1",
            "44444444-4444-4444-4444-444444444444",
            EventTypeRequest {
                event_type: crate::db::EventType::Start,
            },
        )
        .await
        .unwrap();

        let db = state.db.lock().await;
        let subs = db.load_stream_subscriptions().unwrap();
        assert_eq!(subs[0].event_type, crate::db::EventType::Start);
    }

    #[tokio::test]
    async fn app_state_emits_distinct_disconnect_and_terminate_shutdown_signals() {
        let db = Db::open_in_memory().unwrap();
        let (state, mut shutdown_rx) = AppState::new(db, "recv-test".to_owned());

        state.request_disconnect_shutdown();
        shutdown_rx.changed().await.unwrap();
        assert_eq!(*shutdown_rx.borrow(), ShutdownSignal::Disconnect);

        state.request_process_shutdown();
        shutdown_rx.changed().await.unwrap();
        assert_eq!(*shutdown_rx.borrow(), ShutdownSignal::Terminate);
    }

    #[tokio::test]
    async fn admin_clear_data_notifies_dbf_watchers_and_requests_ui_resync() {
        let mut db = Db::open_in_memory().unwrap();
        db.save_profile(
            "https://server.example.com",
            "tok",
            DEFAULT_UPDATE_MODE,
            Some("recv-1"),
        )
        .unwrap();
        db.save_dbf_config(&crate::db::DbfConfig {
            enabled: true,
            path: r"D:\race\output.dbf".to_owned(),
        })
        .unwrap();
        db.save_receiver_mode(&ReceiverMode::Race {
            race_id: "11111111-1111-1111-1111-111111111111".to_owned(),
        })
        .unwrap();

        let (state, _shutdown_rx) = AppState::new(db, "recv-1".to_owned());
        let mut dbf_config_rx = state.dbf_config_rx();
        let mut ui_rx = state.ui_tx.subscribe();

        admin_clear_data(&state).await.unwrap();

        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            dbf_config_rx.changed(),
        )
        .await
        .expect("clear data should notify DBF config subscribers")
        .expect("DBF config channel should stay open");

        let mut saw_resync = false;
        for _ in 0..3 {
            let event = tokio::time::timeout(std::time::Duration::from_millis(100), ui_rx.recv())
                .await
                .expect("clear data should emit UI events")
                .expect("UI event channel should stay open");
            if matches!(event, ReceiverUiEvent::Resync) {
                saw_resync = true;
                break;
            }
        }
        assert!(saw_resync, "clear data should request a UI resync");
    }
}
