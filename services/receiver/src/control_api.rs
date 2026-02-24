//! Localhost control API for the receiver.
//!
//! Binds to 127.0.0.1:9090 (or a caller-supplied address for tests).
//! Routes:
//!   GET  /api/v1/profile        - read current profile
//!   PUT  /api/v1/profile        - update profile
//!   GET  /api/v1/streams        - list streams (merges server + local subs)
//!   GET  /api/v1/subscriptions  - list subscription list
//!   PUT  /api/v1/subscriptions  - replace subscription list
//!   GET  /api/v1/status         - runtime status
//!   GET  /api/v1/logs           - recent log entries
//!   POST /api/v1/connect        - initiate WS connection (async, 202)
//!   POST /api/v1/disconnect     - close WS connection (async, 202)
//!   GET  /api/v1/events         - SSE stream of receiver UI events
//!   POST /api/v1/admin/cursors/reset - reset one stream cursor

use crate::db::{Db, Subscription};
use crate::ui_events::ReceiverUiEvent;
use axum::routing::{get, post};
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json, Router,
};
use rt_protocol::{ReceiverSetSelection, ReplayPolicy};
use rt_updater::workflow::{run_check, run_download, RealChecker, WorkflowState};
use rt_updater::UpdateStatus;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::{broadcast, watch, Mutex, RwLock};
use tracing::warn;

const ADMIN_INTENT_HEADER: &str = "x-rt-receiver-admin-intent";
const ADMIN_RESET_CURSOR_INTENT: &str = "reset-stream-cursor";

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

pub struct AppState {
    pub db: Arc<Mutex<Db>>,
    pub connection_state: Arc<RwLock<ConnectionState>>,
    pub logger: Arc<rt_ui_log::UiLogger<ReceiverUiEvent>>,
    pub shutdown_tx: watch::Sender<bool>,
    pub upstream_url: Arc<RwLock<Option<String>>>,
    pub ui_tx: broadcast::Sender<ReceiverUiEvent>,
    pub update_status: Arc<RwLock<UpdateStatus>>,
    pub staged_update_path: Arc<RwLock<Option<PathBuf>>>,
    pub update_mode: Arc<RwLock<rt_updater::UpdateMode>>,
    pub stream_counts: crate::cache::StreamCounts,
    connect_attempt: AtomicU64,
    retry_streak: AtomicU64,
}

impl AppState {
    pub fn new(db: Db) -> (Arc<Self>, watch::Receiver<bool>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (ui_tx, _) = broadcast::channel(256);
        let state = Arc::new(Self {
            db: Arc::new(Mutex::new(db)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            logger: Arc::new(rt_ui_log::UiLogger::with_buffer(
                ui_tx.clone(),
                |entry| ReceiverUiEvent::LogEntry { entry },
                500,
            )),
            shutdown_tx,
            upstream_url: Arc::new(RwLock::new(None)),
            ui_tx,
            update_status: Arc::new(RwLock::new(UpdateStatus::UpToDate)),
            staged_update_path: Arc::new(RwLock::new(None)),
            update_mode: Arc::new(RwLock::new(rt_updater::UpdateMode::default())),
            stream_counts: crate::cache::StreamCounts::new(),
            connect_attempt: AtomicU64::new(0),
            retry_streak: AtomicU64::new(0),
        });
        (state, shutdown_rx)
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
        self.connect_attempt.fetch_add(1, Ordering::SeqCst);
        self.set_connection_state(ConnectionState::Connecting).await;
    }

    pub async fn request_retry_connect(&self) {
        self.retry_streak.fetch_add(1, Ordering::SeqCst);
        self.connect_attempt.fetch_add(1, Ordering::SeqCst);
        self.set_connection_state(ConnectionState::Connecting).await;
    }

    pub async fn request_reconnect_if_connected(&self) -> bool {
        {
            let mut connection_state = self.connection_state.write().await;
            if *connection_state != ConnectionState::Connected {
                return false;
            }
            self.retry_streak.fetch_add(1, Ordering::SeqCst);
            self.connect_attempt.fetch_add(1, Ordering::SeqCst);
            *connection_state = ConnectionState::Connecting;
        }
        self.emit_connection_state_side_effects(ConnectionState::Connecting)
            .await;
        true
    }

    async fn emit_connection_state_side_effects(&self, new_state: ConnectionState) {
        let streams_count = {
            let db = self.db.lock().await;
            db.load_subscriptions().map(|s| s.len()).unwrap_or(0)
        };
        let _ = self.ui_tx.send(ReceiverUiEvent::StatusChanged {
            connection_state: new_state.clone(),
            streams_count,
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
        *self.connection_state.write().await = new_state.clone();
        self.emit_connection_state_side_effects(new_state).await;
    }

    /// Build the merged streams response from local subscriptions and upstream server.
    pub async fn build_streams_response(&self) -> StreamsResponse {
        let counts_snapshot = self.stream_counts.snapshot();
        let db = self.db.lock().await;
        let subs = match db.load_subscriptions() {
            Ok(s) => s,
            Err(_) => {
                return StreamsResponse {
                    streams: vec![],
                    degraded: true,
                    upstream_error: Some("failed to load subscriptions".to_owned()),
                };
            }
        };
        drop(db);

        let sub_map: HashMap<(&str, &str), &Subscription> = subs
            .iter()
            .map(|s| ((s.forwarder_id.as_str(), s.reader_ip.as_str()), s))
            .collect();

        let upstream_url = self.upstream_url.read().await.clone();
        let conn_state = self.connection_state.read().await.clone();

        let (server_streams, upstream_error) = match (&upstream_url, &conn_state) {
            (None, _) => (None, Some("no profile configured".to_owned())),
            (_, cs) if *cs != ConnectionState::Connected => {
                (None, Some(format!("connection state: {cs:?}")))
            }
            (Some(url), _) => match fetch_server_streams(url).await {
                Ok(streams) => (Some(streams), None),
                Err(e) => {
                    warn!(error = %e, "failed to fetch server streams");
                    (None, Some(e))
                }
            },
        };

        let mut streams: Vec<StreamEntry> = Vec::new();
        let mut seen: HashSet<(String, String)> = HashSet::new();

        if let Some(ref server_streams) = server_streams {
            for si in server_streams {
                let key = (si.forwarder_id.clone(), si.reader_ip.clone());
                let local = sub_map.get(&(si.forwarder_id.as_str(), si.reader_ip.as_str()));
                let port = local.and_then(|s| {
                    s.local_port_override
                        .or_else(|| crate::ports::default_port(&s.reader_ip))
                });
                let sk =
                    crate::cache::StreamKey::new(si.forwarder_id.as_str(), si.reader_ip.as_str());
                let counts = if local.is_some() {
                    counts_snapshot.get(&sk)
                } else {
                    None
                };
                streams.push(StreamEntry {
                    forwarder_id: si.forwarder_id.clone(),
                    reader_ip: si.reader_ip.clone(),
                    subscribed: local.is_some(),
                    local_port: port,
                    online: Some(si.online),
                    display_alias: si.display_alias.clone(),
                    stream_epoch: Some(si.stream_epoch),
                    current_epoch_name: si.current_epoch_name.clone(),
                    reads_total: counts.as_ref().map(|c| c.total),
                    reads_epoch: counts.as_ref().map(|c| c.epoch),
                });
                seen.insert(key);
            }
        }

        for sub in &subs {
            if seen.contains(&(sub.forwarder_id.clone(), sub.reader_ip.clone())) {
                continue;
            }
            let port = sub
                .local_port_override
                .or_else(|| crate::ports::default_port(&sub.reader_ip));
            let sk =
                crate::cache::StreamKey::new(sub.forwarder_id.as_str(), sub.reader_ip.as_str());
            let counts = counts_snapshot.get(&sk);
            streams.push(StreamEntry {
                forwarder_id: sub.forwarder_id.clone(),
                reader_ip: sub.reader_ip.clone(),
                subscribed: true,
                local_port: port,
                online: None,
                display_alias: None,
                stream_epoch: None,
                current_epoch_name: None,
                reads_total: counts.as_ref().map(|c| c.total),
                reads_epoch: counts.as_ref().map(|c| c.epoch),
            });
        }

        let degraded = upstream_error.is_some();
        StreamsResponse {
            streams,
            degraded,
            upstream_error,
        }
    }

    /// Build and broadcast a streams snapshot to SSE clients.
    pub async fn emit_streams_snapshot(&self) {
        let response = self.build_streams_response().await;
        let _ = self.ui_tx.send(ReceiverUiEvent::StreamsSnapshot {
            streams: response.streams,
            degraded: response.degraded,
            upstream_error: response.upstream_error,
        });
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
    pub update_mode: Option<String>,
}

fn default_update_mode() -> String {
    "check-and-download".to_owned()
}

fn default_selection_body() -> ReceiverSetSelection {
    ReceiverSetSelection {
        selection: rt_protocol::ReceiverSelection::Manual {
            streams: Vec::new(),
        },
        replay_policy: ReplayPolicy::Resume,
        replay_targets: None,
    }
}

#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    pub server_url: String,
    pub token: String,
    pub update_mode: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubscriptionRequest {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub local_port_override: Option<u16>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SubscriptionsBody {
    pub subscriptions: Vec<SubscriptionRequest>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CursorResetRequest {
    pub forwarder_id: String,
    pub reader_ip: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct StreamEntry {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub subscribed: bool,
    pub local_port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_epoch_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reads_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reads_epoch: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StreamsResponse {
    pub streams: Vec<StreamEntry>,
    pub degraded: bool,
    pub upstream_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub connection_state: ConnectionState,
    pub local_ok: bool,
    pub streams_count: usize,
}

#[derive(Debug, Serialize)]
pub struct LogsResponse {
    pub entries: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ReplayTargetEpochsQuery {
    forwarder_id: String,
    reader_ip: String,
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

#[derive(Debug, Deserialize)]
struct UpstreamStreamEpochOption {
    epoch: i64,
    name: Option<String>,
    first_event_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UpstreamRacesResponse {
    races: Vec<UpstreamRaceEntry>,
}

#[derive(Debug, Deserialize)]
struct UpstreamRaceEntry {
    race_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct UpstreamRaceEpochMappingsResponse {
    mappings: Vec<UpstreamRaceEpochMapping>,
}

#[derive(Debug, Deserialize)]
struct UpstreamRaceEpochMapping {
    stream_id: String,
    stream_epoch: i64,
}

// ---------------------------------------------------------------------------
// Server stream fetching helpers
// ---------------------------------------------------------------------------

/// Response shape from the server's `GET /api/v1/streams`.
#[derive(Debug, Deserialize)]
struct ServerStreamsResponse {
    streams: Vec<UpstreamStreamInfo>,
}

#[derive(Debug, Deserialize)]
struct UpstreamStreamInfo {
    stream_id: String,
    forwarder_id: String,
    reader_ip: String,
    display_alias: Option<String>,
    stream_epoch: u64,
    online: bool,
    current_epoch_name: Option<String>,
}

/// Normalize a server URL by prepending `ws://` if no scheme is present.
pub(crate) fn normalize_server_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        trimmed.to_owned()
    } else {
        format!("ws://{trimmed}")
    }
}

/// Derive the HTTP base URL from the stored server base URL.
///
/// `ws://host:port`  → `http://host:port`
/// `wss://host:port` → `https://host:port`
pub(crate) fn http_base_url(base_url: &str) -> Option<String> {
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

/// Fetch available streams from the upstream server.
async fn fetch_server_streams(ws_url: &str) -> Result<Vec<UpstreamStreamInfo>, String> {
    let base = http_base_url(ws_url).ok_or_else(|| "cannot parse upstream URL".to_owned())?;
    let url = format!("{base}/api/v1/streams");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("fetch failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }

    let body: ServerStreamsResponse = resp
        .json()
        .await
        .map_err(|e| format!("invalid JSON: {e}"))?;

    Ok(body.streams)
}

// ---------------------------------------------------------------------------
// Route handlers
// ---------------------------------------------------------------------------

async fn get_profile(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db.load_profile() {
        Ok(Some(p)) => Json(ProfileResponse {
            server_url: p.server_url,
            token: p.token,
            update_mode: p.update_mode,
        })
        .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no profile").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_selection(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db.load_receiver_selection() {
        Ok(selection) => Json(selection).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn put_profile(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ProfileRequest>,
) -> impl IntoResponse {
    let url = normalize_server_url(&body.server_url);
    let db = state.db.lock().await;
    let mut effective_update_mode = body.update_mode.clone().unwrap_or_else(|| {
        db.load_profile()
            .ok()
            .flatten()
            .map(|p| p.update_mode)
            .unwrap_or_else(default_update_mode)
    });

    let parsed_update_mode = match serde_json::from_value::<rt_updater::UpdateMode>(
        serde_json::Value::String(effective_update_mode.clone()),
    ) {
        Ok(mode) => mode,
        Err(_) if body.update_mode.is_none() => {
            effective_update_mode = default_update_mode();
            rt_updater::UpdateMode::default()
        }
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                format!(
                    "update_mode must be 'disabled', 'check-only', or 'check-and-download', got '{}'",
                    effective_update_mode
                ),
            )
                .into_response();
        }
    };

    match db.save_profile(&url, &body.token, &effective_update_mode) {
        Ok(()) => {
            drop(db);
            *state.upstream_url.write().await = Some(url);
            *state.update_mode.write().await = parsed_update_mode;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn put_selection(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReceiverSetSelection>,
) -> impl IntoResponse {
    if body.replay_policy == ReplayPolicy::Targeted
        && body
            .replay_targets
            .as_ref()
            .is_none_or(std::vec::Vec::is_empty)
    {
        return (
            StatusCode::BAD_REQUEST,
            "replay_targets must be provided when replay_policy is targeted",
        )
            .into_response();
    }

    if let rt_protocol::ReceiverSelection::Race { ref race_id, .. } = body.selection
        && race_id.trim().is_empty()
    {
        return (
            StatusCode::BAD_REQUEST,
            "race_id must not be empty when mode is race",
        )
            .into_response();
    }

    let normalized = ReceiverSetSelection {
        selection: body.selection,
        replay_policy: body.replay_policy,
        replay_targets: if body.replay_policy == ReplayPolicy::Targeted {
            body.replay_targets
        } else {
            None
        },
    };

    let db = state.db.lock().await;
    match db.save_receiver_selection(&normalized) {
        Ok(()) => {
            drop(db);
            let current_state = state.connection_state.read().await.clone();
            if matches!(
                current_state,
                ConnectionState::Connected | ConnectionState::Connecting
            ) {
                state.request_connect().await;
            }
            StatusCode::NO_CONTENT.into_response()
        }
        Err(crate::db::DbError::ProfileMissing) => {
            let defaults = default_selection_body();
            if normalized == defaults {
                StatusCode::NO_CONTENT.into_response()
            } else {
                (StatusCode::NOT_FOUND, "no profile").into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_streams(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.build_streams_response().await).into_response()
}

async fn get_races(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let profile = {
        let db = state.db.lock().await;
        match db.load_profile() {
            Ok(Some(p)) => p,
            Ok(None) => return (StatusCode::NOT_FOUND, "no profile").into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    };

    let Some(base) = http_base_url(&profile.server_url) else {
        return (StatusCode::BAD_REQUEST, "invalid upstream URL").into_response();
    };
    let url = format!("{base}/api/v1/races");

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("HTTP client error: {e}"),
            )
                .into_response();
        }
    };

    let response = match client.get(&url).bearer_auth(profile.token).send().await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("fetch failed: {e}")).into_response(),
    };

    let status =
        StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    match response.json::<serde_json::Value>().await {
        Ok(body) => (status, Json(body)).into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("invalid JSON from upstream: {e}"),
        )
            .into_response(),
    }
}

async fn get_replay_target_epochs(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ReplayTargetEpochsQuery>,
) -> impl IntoResponse {
    let profile = {
        let db = state.db.lock().await;
        match db.load_profile() {
            Ok(Some(p)) => p,
            Ok(None) => return (StatusCode::NOT_FOUND, "no profile").into_response(),
            Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        }
    };

    let Some(base) = http_base_url(&profile.server_url) else {
        return (StatusCode::BAD_REQUEST, "invalid upstream URL").into_response();
    };

    let client = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("HTTP client error: {e}"),
            )
                .into_response();
        }
    };

    let streams_url = format!("{base}/api/v1/streams");
    let streams_response = match client
        .get(&streams_url)
        .bearer_auth(&profile.token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("fetch failed: {e}")).into_response(),
    };
    if !streams_response.status().is_success() {
        return (
            StatusCode::BAD_GATEWAY,
            format!("upstream streams returned {}", streams_response.status()),
        )
            .into_response();
    }
    let upstream_streams = match streams_response.json::<ServerStreamsResponse>().await {
        Ok(body) => body.streams,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("invalid streams JSON from upstream: {e}"),
            )
                .into_response();
        }
    };
    let Some(stream) = upstream_streams.iter().find(|stream| {
        stream.forwarder_id == query.forwarder_id && stream.reader_ip == query.reader_ip
    }) else {
        return (StatusCode::NOT_FOUND, "stream not found").into_response();
    };

    let epochs_url = format!("{base}/api/v1/streams/{}/epochs", stream.stream_id);
    let epochs_response = match client
        .get(&epochs_url)
        .bearer_auth(&profile.token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("fetch failed: {e}")).into_response(),
    };
    if !epochs_response.status().is_success() {
        return (
            StatusCode::BAD_GATEWAY,
            format!("upstream epochs returned {}", epochs_response.status()),
        )
            .into_response();
    }
    let upstream_epochs = match epochs_response
        .json::<Vec<UpstreamStreamEpochOption>>()
        .await
    {
        Ok(body) => body,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("invalid epochs JSON from upstream: {e}"),
            )
                .into_response();
        }
    };

    let races_url = format!("{base}/api/v1/races");
    let races_response = match client
        .get(&races_url)
        .bearer_auth(&profile.token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("fetch failed: {e}")).into_response(),
    };
    if !races_response.status().is_success() {
        return (
            StatusCode::BAD_GATEWAY,
            format!("upstream races returned {}", races_response.status()),
        )
            .into_response();
    }
    let races = match races_response.json::<UpstreamRacesResponse>().await {
        Ok(body) => body.races,
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                format!("invalid races JSON from upstream: {e}"),
            )
                .into_response();
        }
    };

    let race_mapping_fetches = races.iter().map(|race| {
        let client = client.clone();
        let base = base.clone();
        let token = profile.token.clone();
        let race_id = race.race_id.clone();
        let race_name = race.name.clone();
        async move {
            let mappings_url = format!("{base}/api/v1/races/{race_id}/stream-epochs");
            let mappings_response = match client.get(&mappings_url).bearer_auth(&token).send().await
            {
                Ok(r) => r,
                Err(e) => {
                    return Err((StatusCode::BAD_GATEWAY, format!("fetch failed: {e}")));
                }
            };
            if !mappings_response.status().is_success() {
                return Err((
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "upstream stream-epochs for race {} returned {}",
                        race_id,
                        mappings_response.status()
                    ),
                ));
            }
            let mappings = match mappings_response
                .json::<UpstreamRaceEpochMappingsResponse>()
                .await
            {
                Ok(body) => body.mappings,
                Err(e) => {
                    return Err((
                        StatusCode::BAD_GATEWAY,
                        format!("invalid stream-epochs JSON from upstream: {e}"),
                    ));
                }
            };
            Ok((race_name, mappings))
        }
    });

    let race_mappings = futures_util::future::join_all(race_mapping_fetches).await;
    let mut race_names_by_epoch: HashMap<i64, BTreeSet<String>> = HashMap::new();
    for race_mappings_result in race_mappings {
        let (race_name, mappings) = match race_mappings_result {
            Ok(value) => value,
            Err((status, message)) => return (status, message).into_response(),
        };
        for mapping in mappings {
            if mapping.stream_id == stream.stream_id {
                race_names_by_epoch
                    .entry(mapping.stream_epoch)
                    .or_default()
                    .insert(race_name.clone());
            }
        }
    }

    let epochs = upstream_epochs
        .into_iter()
        .map(|epoch| ReplayTargetEpochOption {
            stream_epoch: epoch.epoch,
            name: epoch.name,
            first_seen_at: epoch.first_event_at,
            race_names: race_names_by_epoch
                .remove(&epoch.epoch)
                .map_or_else(Vec::new, |names| names.into_iter().collect()),
        })
        .collect();

    Json(ReplayTargetEpochsResponse { epochs }).into_response()
}

async fn put_subscriptions(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SubscriptionsBody>,
) -> impl IntoResponse {
    let mut seen = std::collections::HashSet::new();
    for s in &body.subscriptions {
        if !seen.insert((s.forwarder_id.clone(), s.reader_ip.clone())) {
            return (StatusCode::BAD_REQUEST, "duplicate subscriptions").into_response();
        }
    }

    let subs: Vec<Subscription> = body
        .subscriptions
        .into_iter()
        .map(|s| Subscription {
            forwarder_id: s.forwarder_id,
            reader_ip: s.reader_ip,
            local_port_override: s.local_port_override,
        })
        .collect();
    let mut db = state.db.lock().await;
    match db.replace_subscriptions(&subs) {
        Ok(()) => {
            drop(db);
            let conn = state.connection_state.read().await.clone();
            let db = state.db.lock().await;
            let streams_count = db.load_subscriptions().map(|s| s.len()).unwrap_or(0);
            let _ = state.ui_tx.send(ReceiverUiEvent::StatusChanged {
                connection_state: conn,
                streams_count,
            });
            drop(db);
            state.emit_streams_snapshot().await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_subscriptions(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db.load_subscriptions() {
        Ok(subscriptions) => Json(SubscriptionsBody {
            subscriptions: subscriptions
                .into_iter()
                .map(|s| SubscriptionRequest {
                    forwarder_id: s.forwarder_id,
                    reader_ip: s.reader_ip,
                    local_port_override: s.local_port_override,
                })
                .collect(),
        })
        .into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = state.connection_state.read().await.clone();
    let db = state.db.lock().await;
    let streams_count = db.load_subscriptions().map(|s| s.len()).unwrap_or(0);
    let local_ok = db.integrity_check().is_ok();
    Json(StatusResponse {
        connection_state: conn,
        local_ok,
        streams_count,
    })
    .into_response()
}

async fn get_logs(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let entries = state.logger.entries();
    Json(LogsResponse { entries }).into_response()
}

async fn post_connect(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let current = state.connection_state.read().await.clone();
    if current == ConnectionState::Connected {
        return StatusCode::OK.into_response();
    }
    state.request_connect().await;
    StatusCode::ACCEPTED.into_response()
}

async fn post_disconnect(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let current = state.connection_state.read().await.clone();
    if current == ConnectionState::Disconnected {
        return StatusCode::OK.into_response();
    }
    state
        .set_connection_state(ConnectionState::Disconnecting)
        .await;
    let _ = state.shutdown_tx.send(true);
    StatusCode::ACCEPTED.into_response()
}

async fn post_admin_reset_cursor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CursorResetRequest>,
) -> impl IntoResponse {
    let has_valid_intent = headers
        .get(ADMIN_INTENT_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == ADMIN_RESET_CURSOR_INTENT);
    if !has_valid_intent {
        return (StatusCode::FORBIDDEN, "missing or invalid admin intent").into_response();
    }

    let db = state.db.lock().await;
    match db.delete_cursor(&body.forwarder_id, &body.reader_ip) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_update_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let status = state.update_status.read().await.clone();
    Json(status).into_response()
}

async fn post_update_apply(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let path = state.staged_update_path.read().await.clone();
    match path {
        Some(path) => {
            // Detect a missing staged file synchronously so the status update is
            // immediate and reliable (avoids Windows blocking-thread startup
            // latency causing the test to race against a fixed timeout).
            let staged_file_error = match path.try_exists() {
                Ok(true) => None,
                Ok(false) => Some(format!("staged file not found: {}", path.display())),
                Err(e) => Some(format!(
                    "failed to access staged file {}: {}",
                    path.display(),
                    e
                )),
            };
            if let Some(error) = staged_file_error {
                tracing::error!(path = %path.display(), error = %error, "cannot apply update");
                *state.update_status.write().await = rt_updater::UpdateStatus::Failed { error };
                return (
                    StatusCode::OK,
                    Json(serde_json::json!({"status": "failed"})),
                )
                    .into_response();
            }
            let state_clone = Arc::clone(&state);
            // Send response before exiting
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                match tokio::task::spawn_blocking(move || {
                    rt_updater::UpdateChecker::apply_and_exit(&path)
                })
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::error!(error = %e, "update apply failed");
                        *state_clone.update_status.write().await =
                            rt_updater::UpdateStatus::Failed {
                                error: e.to_string(),
                            };
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "update apply task failed");
                        *state_clone.update_status.write().await =
                            rt_updater::UpdateStatus::Failed {
                                error: e.to_string(),
                            };
                    }
                }
            });
            (
                StatusCode::OK,
                Json(serde_json::json!({"status": "applying"})),
            )
                .into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "no update staged"})),
        )
            .into_response(),
    }
}

async fn post_update_check(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let update_mode = *state.update_mode.read().await;

    let checker = match rt_updater::UpdateChecker::new(
        "iwismer",
        "rusty-timer",
        "receiver",
        env!("CARGO_PKG_VERSION"),
    ) {
        Ok(c) => RealChecker::new(c),
        Err(e) => {
            let (status_code, status) = update_check_init_error_status(e.to_string());
            *state.update_status.write().await = status.clone();
            return (status_code, Json(status)).into_response();
        }
    };

    let workflow_state = ReceiverWorkflowAdapter::new(Arc::clone(&state));
    let status = run_check(&workflow_state, &checker, update_mode).await;
    Json(status).into_response()
}

fn update_check_init_error_status(error: String) -> (StatusCode, UpdateStatus) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        UpdateStatus::Failed { error },
    )
}

struct ReceiverWorkflowAdapter {
    state: Arc<AppState>,
}

impl ReceiverWorkflowAdapter {
    fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

impl WorkflowState for ReceiverWorkflowAdapter {
    fn current_status<'a>(
        &'a self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = UpdateStatus> + Send + 'a>> {
        Box::pin(async move { self.state.update_status.read().await.clone() })
    }

    fn set_status<'a>(
        &'a self,
        status: UpdateStatus,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            *self.state.update_status.write().await = status;
        })
    }

    fn set_downloaded<'a>(
        &'a self,
        status: UpdateStatus,
        path: PathBuf,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            *self.state.update_status.write().await = status;
            *self.state.staged_update_path.write().await = Some(path);
        })
    }

    fn emit_status_changed<'a>(
        &'a self,
        status: UpdateStatus,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let _ = self
                .state
                .ui_tx
                .send(crate::ui_events::ReceiverUiEvent::UpdateStatusChanged {
                    status: status.clone(),
                });
        })
    }
}

async fn post_update_download(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let checker = match rt_updater::UpdateChecker::new(
        "iwismer",
        "rusty-timer",
        "receiver",
        env!("CARGO_PKG_VERSION"),
    ) {
        Ok(c) => RealChecker::new(c),
        Err(e) => {
            let status = rt_updater::UpdateStatus::Failed {
                error: e.to_string(),
            };
            *state.update_status.write().await = status.clone();
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(status)).into_response();
        }
    };

    let workflow_state = ReceiverWorkflowAdapter::new(Arc::clone(&state));
    match run_download(&workflow_state, &checker).await {
        Ok(status) => Json(status).into_response(),
        Err(status) => (StatusCode::CONFLICT, Json(status)).into_response(),
    }
}

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/profile", get(get_profile).put(put_profile))
        .route("/api/v1/selection", get(get_selection).put(put_selection))
        .route("/api/v1/streams", get(get_streams))
        .route("/api/v1/races", get(get_races))
        .route(
            "/api/v1/replay-targets/epochs",
            get(get_replay_target_epochs),
        )
        .route(
            "/api/v1/subscriptions",
            get(get_subscriptions).put(put_subscriptions),
        )
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/logs", get(get_logs))
        .route("/api/v1/connect", post(post_connect))
        .route("/api/v1/disconnect", post(post_disconnect))
        .route("/api/v1/events", get(crate::sse::receiver_sse))
        .route("/api/v1/admin/cursors/reset", post(post_admin_reset_cursor))
        .route("/api/v1/update/status", get(get_update_status))
        .route("/api/v1/update/apply", post(post_update_apply))
        .route("/api/v1/update/check", post(post_update_check))
        .route("/api/v1/update/download", post(post_update_download))
        .fallback(crate::ui_server::serve_ui)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use rt_updater::workflow::{run_check, run_download, Checker};
    use std::future::Future;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

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

    #[test]
    fn http_base_url_ws_with_port() {
        assert_eq!(
            http_base_url("ws://127.0.0.1:8080"),
            Some("http://127.0.0.1:8080".to_owned())
        );
    }

    #[test]
    fn http_base_url_wss_with_port() {
        assert_eq!(
            http_base_url("wss://server.example.com:8443"),
            Some("https://server.example.com:8443".to_owned())
        );
    }

    #[test]
    fn http_base_url_wss_no_port() {
        assert_eq!(
            http_base_url("wss://server.example.com"),
            Some("https://server.example.com".to_owned())
        );
    }

    #[test]
    fn http_base_url_invalid_scheme() {
        assert_eq!(http_base_url("http://server.example.com"), None);
    }

    #[test]
    fn http_base_url_invalid_url() {
        assert_eq!(http_base_url("not a url"), None);
    }

    #[test]
    fn normalize_prepends_ws_when_no_scheme() {
        assert_eq!(
            normalize_server_url("127.0.0.1:8080"),
            "ws://127.0.0.1:8080"
        );
    }

    #[test]
    fn normalize_preserves_ws_scheme() {
        assert_eq!(
            normalize_server_url("ws://127.0.0.1:8080"),
            "ws://127.0.0.1:8080"
        );
    }

    #[test]
    fn normalize_preserves_wss_scheme() {
        assert_eq!(
            normalize_server_url("wss://server.example.com"),
            "wss://server.example.com"
        );
    }

    #[test]
    fn normalize_strips_trailing_slash() {
        assert_eq!(
            normalize_server_url("ws://127.0.0.1:8080/"),
            "ws://127.0.0.1:8080"
        );
    }

    #[test]
    fn normalize_trims_whitespace() {
        assert_eq!(
            normalize_server_url("  127.0.0.1:8080  "),
            "ws://127.0.0.1:8080"
        );
    }

    #[tokio::test]
    async fn post_update_apply_sets_failed_status_when_staged_file_missing() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);
        *state.update_status.write().await = UpdateStatus::Downloaded {
            version: "1.2.3".to_owned(),
        };
        *state.staged_update_path.write().await = Some(temp.path().join("missing-staged-receiver"));

        let app = build_router(Arc::clone(&state));

        let apply_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/update/apply")
                    .body(Body::empty())
                    .expect("build apply request"),
            )
            .await
            .expect("apply request");
        assert_eq!(apply_resp.status(), StatusCode::OK);

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;

        let status_resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/update/status")
                    .body(Body::empty())
                    .expect("build status request"),
            )
            .await
            .expect("status request");

        let bytes = status_resp
            .into_body()
            .collect()
            .await
            .expect("collect body");
        let json: serde_json::Value =
            serde_json::from_slice(&bytes.to_bytes()).expect("status json");
        assert_eq!(json["status"], "failed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn post_update_apply_sets_failed_status_when_staged_file_access_denied() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);
        let blocked_dir = temp.path().join("blocked");
        std::fs::create_dir(&blocked_dir).expect("create blocked dir");
        let mut perms = std::fs::metadata(&blocked_dir)
            .expect("metadata")
            .permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&blocked_dir, perms).expect("set blocked permissions");

        *state.update_status.write().await = UpdateStatus::Downloaded {
            version: "1.2.3".to_owned(),
        };
        *state.staged_update_path.write().await = Some(blocked_dir.join("staged-receiver"));

        let app = build_router(Arc::clone(&state));

        let apply_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/update/apply")
                    .body(Body::empty())
                    .expect("build apply request"),
            )
            .await
            .expect("apply request");
        assert_eq!(apply_resp.status(), StatusCode::OK);

        let mut restore_perms = std::fs::metadata(&blocked_dir)
            .expect("metadata")
            .permissions();
        restore_perms.set_mode(0o700);
        std::fs::set_permissions(&blocked_dir, restore_perms).expect("restore permissions");

        let status_resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/update/status")
                    .body(Body::empty())
                    .expect("build status request"),
            )
            .await
            .expect("status request");

        let bytes = status_resp
            .into_body()
            .collect()
            .await
            .expect("collect body");
        let json: serde_json::Value =
            serde_json::from_slice(&bytes.to_bytes()).expect("status json");
        assert_eq!(json["status"], "failed");
        let error = json["error"]
            .as_str()
            .expect("error string")
            .to_ascii_lowercase();
        assert!(
            error.contains("permission denied"),
            "expected permission error, got: {error}"
        );
    }

    #[tokio::test]
    async fn get_update_status_serializes_variants() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);
        let app = build_router(Arc::clone(&state));

        let up_to_date_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/update/status")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("up_to_date response");
        let up_to_date_body = up_to_date_resp
            .into_body()
            .collect()
            .await
            .expect("collect up_to_date body");
        let up_to_date_json: serde_json::Value =
            serde_json::from_slice(&up_to_date_body.to_bytes()).expect("up_to_date json");
        assert_eq!(up_to_date_json["status"], "up_to_date");

        *state.update_status.write().await = UpdateStatus::Downloaded {
            version: "1.2.3".to_owned(),
        };
        let downloaded_resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/update/status")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("downloaded response");
        let downloaded_body = downloaded_resp
            .into_body()
            .collect()
            .await
            .expect("collect downloaded body");
        let downloaded_json: serde_json::Value =
            serde_json::from_slice(&downloaded_body.to_bytes()).expect("downloaded json");
        assert_eq!(downloaded_json["status"], "downloaded");
        assert_eq!(downloaded_json["version"], "1.2.3");

        *state.update_status.write().await = UpdateStatus::Failed {
            error: "boom".to_owned(),
        };
        let failed_resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/update/status")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("failed response");
        let failed_body = failed_resp
            .into_body()
            .collect()
            .await
            .expect("collect failed body");
        let failed_json: serde_json::Value =
            serde_json::from_slice(&failed_body.to_bytes()).expect("failed json");
        assert_eq!(failed_json["status"], "failed");
        assert_eq!(failed_json["error"], "boom");
    }

    #[tokio::test]
    async fn post_update_apply_returns_not_found_when_unstaged() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/update/apply")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = response.into_body().collect().await.expect("collect body");
        let json: serde_json::Value = serde_json::from_slice(&body.to_bytes()).expect("json body");
        assert_eq!(json["error"], "no update staged");
    }

    #[tokio::test]
    async fn update_check_skips_download_in_check_only_mode() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);
        let download_calls = Arc::new(AtomicUsize::new(0));
        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::Available {
                version: "1.2.3".to_owned(),
            }),
            download_result: Ok(std::path::PathBuf::from("/tmp/unused")),
            download_calls: Arc::clone(&download_calls),
        };

        let workflow_state = ReceiverWorkflowAdapter::new(Arc::clone(&state));
        let status = run_check(&workflow_state, &checker, rt_updater::UpdateMode::CheckOnly).await;

        assert_eq!(
            status,
            UpdateStatus::Available {
                version: "1.2.3".to_owned()
            }
        );
        assert_eq!(download_calls.load(Ordering::SeqCst), 0);
        assert!(state.staged_update_path.read().await.is_none());
    }

    #[tokio::test]
    async fn update_check_downloads_in_check_and_download_mode() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);
        let download_calls = Arc::new(AtomicUsize::new(0));
        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::Available {
                version: "1.2.3".to_owned(),
            }),
            download_result: Ok(std::path::PathBuf::from("/tmp/staged-receiver")),
            download_calls: Arc::clone(&download_calls),
        };

        let workflow_state = ReceiverWorkflowAdapter::new(Arc::clone(&state));
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
            *state.staged_update_path.read().await,
            Some(std::path::PathBuf::from("/tmp/staged-receiver"))
        );
    }

    #[tokio::test]
    async fn update_download_downloads_when_available() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);
        *state.update_status.write().await = UpdateStatus::Available {
            version: "2.0.0".to_owned(),
        };
        let download_calls = Arc::new(AtomicUsize::new(0));
        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(std::path::PathBuf::from("/tmp/staged-receiver")),
            download_calls: Arc::clone(&download_calls),
        };

        let workflow_state = ReceiverWorkflowAdapter::new(Arc::clone(&state));
        let status = run_download(&workflow_state, &checker).await;

        assert_eq!(
            status,
            Ok(UpdateStatus::Downloaded {
                version: "2.0.0".to_owned()
            })
        );
        assert_eq!(download_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            *state.staged_update_path.read().await,
            Some(std::path::PathBuf::from("/tmp/staged-receiver"))
        );
    }

    #[tokio::test]
    async fn update_download_failure_emits_failed_event() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);
        *state.update_status.write().await = UpdateStatus::Available {
            version: "2.0.0".to_owned(),
        };
        let mut rx = state.ui_tx.subscribe();

        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Err("boom".to_owned()),
            download_calls: Arc::new(AtomicUsize::new(0)),
        };

        let workflow_state = ReceiverWorkflowAdapter::new(Arc::clone(&state));
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
            crate::ui_events::ReceiverUiEvent::UpdateStatusChanged { status } => match status {
                UpdateStatus::Failed { error } => assert_eq!(error, "boom"),
                other => panic!("unexpected status: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn update_download_returns_conflict_when_up_to_date() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);

        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(std::path::PathBuf::from("/tmp/unused")),
            download_calls: Arc::new(AtomicUsize::new(0)),
        };

        let workflow_state = ReceiverWorkflowAdapter::new(Arc::clone(&state));
        let status = run_download(&workflow_state, &checker).await;
        assert!(status.is_err());
    }

    #[tokio::test]
    async fn update_download_is_idempotent_when_already_downloaded() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);
        *state.update_status.write().await = UpdateStatus::Downloaded {
            version: "2.0.0".to_owned(),
        };

        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(std::path::PathBuf::from("/tmp/unused")),
            download_calls: Arc::new(AtomicUsize::new(0)),
        };

        let workflow_state = ReceiverWorkflowAdapter::new(Arc::clone(&state));
        let status = run_download(&workflow_state, &checker).await;
        assert_eq!(
            status,
            Ok(UpdateStatus::Downloaded {
                version: "2.0.0".to_owned()
            })
        );
    }

    #[tokio::test]
    async fn build_streams_response_includes_reads_fields_for_subscribed_streams() {
        let db = Db::open_in_memory().expect("open in-memory db");
        let (state, _shutdown_rx) = AppState::new(db);
        {
            let mut db = state.db.lock().await;
            db.replace_subscriptions(&[crate::db::Subscription {
                forwarder_id: "f1".to_owned(),
                reader_ip: "10.0.0.1".to_owned(),
                local_port_override: None,
            }])
            .expect("replace subscriptions");
        }

        let key = crate::cache::StreamKey::new("f1", "10.0.0.1");
        for seq in 1..=9 {
            state.stream_counts.record(&key, 7, seq);
        }

        let response = state.build_streams_response().await;
        let stream = response
            .streams
            .iter()
            .find(|s| s.forwarder_id == "f1" && s.reader_ip == "10.0.0.1")
            .expect("stream exists");

        assert_eq!(stream.reads_total, Some(9));
        assert_eq!(stream.reads_epoch, Some(9));
    }

    #[tokio::test]
    async fn build_streams_response_excludes_reads_fields_for_unsubscribed_streams() {
        let db = Db::open_in_memory().expect("open in-memory db");
        let (state, _shutdown_rx) = AppState::new(db);

        let key = crate::cache::StreamKey::new("f1", "10.0.0.1");
        for seq in 1..=5 {
            state.stream_counts.record(&key, 3, seq);
        }

        let upstream_app = Router::new().route(
            "/api/v1/streams",
            get(|| async {
                Json(serde_json::json!({
                    "streams": [{
                        "stream_id": "stream-1",
                        "forwarder_id": "f1",
                        "reader_ip": "10.0.0.1",
                        "display_alias": "Finish",
                        "stream_epoch": 3,
                        "online": true,
                        "current_epoch_name": "Heat 1"
                    }]
                }))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind upstream listener");
        let addr = listener.local_addr().expect("read upstream addr");
        tokio::spawn(async move {
            axum::serve(listener, upstream_app)
                .await
                .expect("serve upstream app");
        });
        *state.upstream_url.write().await = Some(format!("ws://{addr}"));
        state.set_connection_state(ConnectionState::Connected).await;

        let response = state.build_streams_response().await;
        let stream = response
            .streams
            .iter()
            .find(|s| s.forwarder_id == "f1" && s.reader_ip == "10.0.0.1")
            .expect("unsubscribed upstream stream is present");
        assert!(!stream.subscribed);
        assert_eq!(stream.reads_total, None);
        assert_eq!(stream.reads_epoch, None);
    }

    #[test]
    fn update_check_init_error_maps_to_http_500() {
        let (status_code, status) = update_check_init_error_status("boom".to_owned());
        assert_eq!(status_code, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            status,
            UpdateStatus::Failed {
                error: "boom".to_owned()
            }
        );
    }
}
