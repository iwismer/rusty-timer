//! Receiver control API — business logic for the receiver.
//!
//! All handler functions are plain async functions that take `&AppState`
//! and return `Result<T, ReceiverError>`.  The Tauri app wraps these as
//! IPC commands.

use crate::db::{DEFAULT_UPDATE_MODE, Db, StreamSubscription};
use crate::error::ReceiverError;
use crate::ui_events::ReceiverUiEvent;
use rt_domain::ReceiverMode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

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
use tokio::sync::{Mutex, RwLock, broadcast, watch};
use tracing::warn;

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
    connect_attempt: AtomicU64,
    connect_attempt_version: watch::Sender<u64>,
    /// Keepalive receiver to prevent the connect-attempt watch channel from being dropped
    /// when no runtime subscriber is active.
    _connect_attempt_keepalive: watch::Receiver<u64>,
    retry_streak: AtomicU64,
    /// Monotonic counter incremented when DBF config changes; subscribers
    /// (runtime.rs) use this to restart the DBF writer. Use
    /// `notify_dbf_config_changed()` and `dbf_config_rx()` to interact.
    dbf_config_version: watch::Sender<u64>,
    /// Keepalive receiver to prevent the watch channel from being dropped
    /// when no external subscribers exist.
    _dbf_config_keepalive: watch::Receiver<u64>,
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
        let (connect_attempt_version, _connect_attempt_keepalive) = watch::channel(0u64);
        let (dbf_config_version, _dbf_config_keepalive) = watch::channel(0u64);
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
            connect_attempt: AtomicU64::new(0),
            connect_attempt_version,
            _connect_attempt_keepalive,
            retry_streak: AtomicU64::new(0),
            dbf_config_version,
            _dbf_config_keepalive,
        });
        (state, shutdown_rx)
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

    pub fn connect_attempt_rx(&self) -> watch::Receiver<u64> {
        self.connect_attempt_version.subscribe()
    }

    fn bump_connect_attempt(&self) -> u64 {
        let next = self.connect_attempt.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.connect_attempt_version.send(next);
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
        self.bump_connect_attempt();
        self.set_connection_state(ConnectionState::Connecting).await;
    }

    pub async fn request_retry_connect(&self) {
        self.retry_streak.fetch_add(1, Ordering::SeqCst);
        self.bump_connect_attempt();
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
        self.bump_connect_attempt();
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
        drop(db);

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
            let port = sub.local_port_override.or_else(|| {
                sub.reader_ip
                    .as_deref()
                    .and_then(crate::ports::default_port)
            });
            let counts = sub
                .forwarder_id
                .as_deref()
                .zip(sub.reader_ip.as_deref())
                .and_then(|(forwarder_id, reader_ip)| {
                    let sk = crate::cache::StreamKey::new(forwarder_id, reader_ip);
                    counts_snapshot.get(&sk)
                });
            let cursor = cursor_map.get(sub.stream_id.as_str());
            let discovered_stream = discovered_streams
                .get(&(sub.forwarder_endpoint_id.as_str(), sub.stream_id.as_str()));
            streams.push(StreamEntry {
                forwarder_endpoint_id: sub.forwarder_endpoint_id.clone(),
                stream_id: sub.stream_id.clone(),
                forwarder_id: sub.forwarder_id.clone(),
                reader_ip: sub.reader_ip.clone(),
                subscribed: true,
                local_port: port,
                event_type: Some(sub.event_type),
                online: None,
                reader_connected: None,
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
                streams.push(StreamEntry {
                    forwarder_endpoint_id: endpoint_id.clone(),
                    stream_id: stream.stream_id.clone(),
                    forwarder_id: None,
                    reader_ip: None,
                    subscribed: false,
                    local_port: None,
                    event_type: None,
                    online: None,
                    reader_connected: None,
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
    pub server_url: String,
    pub token: String,
    pub receiver_id: String,
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

pub async fn get_profile(state: &AppState) -> Result<ProfileResponse, ReceiverError> {
    let receiver_id = state.receiver_id.read().await.clone();
    let db = state.db.lock().await;
    match db.load_profile() {
        Ok(Some(p)) => Ok(ProfileResponse {
            server_url: p.server_url,
            token: p.token,
            receiver_id,
        }),
        Ok(None) => Err(ReceiverError::NotFound("no profile".to_owned())),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
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
    let persist_receiver_id = new_receiver_id
        .clone()
        .or_else(|| db.load_profile().ok().flatten().and_then(|p| p.receiver_id));

    match db.save_profile(
        &url,
        &body.token,
        DEFAULT_UPDATE_MODE,
        persist_receiver_id.as_deref(),
    ) {
        Ok(()) => {
            drop(db);
            if let Some(id) = new_receiver_id {
                *state.receiver_id.write().await = id;
            }
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
    forwarder_id: String,
    reader_ip: String,
) -> Result<ReplayTargetEpochsResponse, ReceiverError> {
    let db = state.db.lock().await;
    let rows = db
        .load_replay_target_epochs(&forwarder_id, &reader_ip)
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

async fn server_device_status(state: &AppState) -> ServerDeviceStatus {
    let server_url = {
        let db = state.db.lock().await;
        match db.load_profile() {
            Ok(Some(profile)) if !profile.server_url.trim().is_empty() => profile.server_url,
            _ => return ServerDeviceStatus::not_configured(),
        }
    };
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
                forwarder_id: "String",
                reader_ip: "String"
            ) -> "ReplayTargetEpochsResponse",
            get_subscriptions() -> "SubscriptionsBody",
            put_subscriptions(body: "SubscriptionsBody") -> "()",
            get_status() -> "StatusResponse",
            reconnect_server() -> "()",
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
    use crate::db::Db;

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
    async fn reconnect_server_notifies_connect_watchers() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut connect_rx = state.connect_attempt_rx();

        reconnect_server(&state).await.unwrap();

        connect_rx.changed().await.unwrap();
        assert_eq!(*connect_rx.borrow(), 1);
        assert_eq!(state.current_connect_attempt(), 1);
        assert_eq!(
            state.connection_state.borrow().clone(),
            ConnectionState::Connecting
        );
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
