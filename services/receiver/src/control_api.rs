//! Receiver control API — business logic for the receiver.
//!
//! All handler functions are plain async functions that take `&AppState`
//! and return `Result<T, ReceiverError>`.  The Tauri app wraps these as
//! IPC commands.

use crate::db::{DEFAULT_UPDATE_MODE, Db, Subscription};
use crate::error::ReceiverError;
use crate::ui_events::ReceiverUiEvent;
use rt_protocol::ReceiverMode;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
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
    pub upstream_url: Arc<RwLock<Option<String>>>,
    pub ui_tx: broadcast::Sender<ReceiverUiEvent>,
    pub stream_counts: crate::cache::StreamCounts,
    pub receiver_id: Arc<RwLock<String>>,
    pub db_integrity_ok: bool,
    pub http_client: reqwest::Client,
    pub chip_lookup: Arc<tokio::sync::RwLock<crate::session::ChipLookup>>,
    /// Channel for sending WS commands from Tauri handlers into the active session.
    /// `None` when no session is active.
    pub ws_cmd_tx: RwLock<Option<tokio::sync::mpsc::Sender<crate::session::WsCommand>>>,
    connect_attempt: AtomicU64,
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
            upstream_url: Arc::new(RwLock::new(None)),
            ui_tx,
            stream_counts: crate::cache::StreamCounts::new(),
            receiver_id: Arc::new(RwLock::new(receiver_id)),
            db_integrity_ok,
            http_client,
            chip_lookup: Arc::new(tokio::sync::RwLock::new(crate::session::ChipLookup::new())),
            ws_cmd_tx: RwLock::new(None),
            connect_attempt: AtomicU64::new(0),
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
        self.connect_attempt.fetch_add(1, Ordering::SeqCst);
        self.set_connection_state(ConnectionState::Connecting).await;
    }

    pub async fn request_retry_connect(&self) {
        self.retry_streak.fetch_add(1, Ordering::SeqCst);
        self.connect_attempt.fetch_add(1, Ordering::SeqCst);
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
        self.connect_attempt.fetch_add(1, Ordering::SeqCst);
        self.emit_connection_state_side_effects(ConnectionState::Connecting)
            .await;
        true
    }

    async fn emit_connection_state_side_effects(&self, new_state: ConnectionState) {
        let streams_count = {
            let db = self.db.lock().await;
            match db.load_subscriptions() {
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
        let (cursors, cursors_degraded) = match db.load_cursors() {
            Ok(c) => (c, false),
            Err(e) => {
                warn!(error = %e, "failed to load cursors");
                (vec![], true)
            }
        };
        drop(db);

        let cursor_map: HashMap<(&str, &str), &crate::db::CursorRecord> = cursors
            .iter()
            .map(|c| ((c.forwarder_id.as_str(), c.reader_ip.as_str()), c))
            .collect();

        let sub_map: HashMap<(&str, &str), &Subscription> = subs
            .iter()
            .map(|s| ((s.forwarder_id.as_str(), s.reader_ip.as_str()), s))
            .collect();

        let upstream_url = self.upstream_url.read().await.clone();
        let conn_state = self.connection_state.borrow().clone();

        let (server_streams, upstream_error) = match (&upstream_url, &conn_state) {
            (None, _) => (None, Some("no profile configured".to_owned())),
            (_, cs) if *cs != ConnectionState::Connected => {
                (None, Some(format!("connection state: {cs:?}")))
            }
            (Some(url), _) => match fetch_server_streams(&self.http_client, url).await {
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
                let cursor = cursor_map.get(&(si.forwarder_id.as_str(), si.reader_ip.as_str()));
                streams.push(StreamEntry {
                    forwarder_id: si.forwarder_id.clone(),
                    reader_ip: si.reader_ip.clone(),
                    subscribed: local.is_some(),
                    local_port: port,
                    event_type: local.map(|s| s.event_type),
                    online: Some(si.online),
                    reader_connected: Some(si.reader_connected),
                    display_alias: si.display_alias.clone(),
                    stream_epoch: Some(si.stream_epoch),
                    current_epoch_name: si.current_epoch_name.clone(),
                    reads_total: counts.as_ref().map(|c| c.total),
                    reads_epoch: counts.as_ref().map(|c| c.epoch),
                    cursor_epoch: cursor.map(|c| c.stream_epoch),
                    cursor_seq: cursor.map(|c| c.last_seq),
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
            let cursor = cursor_map.get(&(sub.forwarder_id.as_str(), sub.reader_ip.as_str()));
            streams.push(StreamEntry {
                forwarder_id: sub.forwarder_id.clone(),
                reader_ip: sub.reader_ip.clone(),
                subscribed: true,
                local_port: port,
                event_type: Some(sub.event_type),
                online: None,
                reader_connected: None,
                display_alias: None,
                stream_epoch: None,
                current_epoch_name: None,
                reads_total: counts.as_ref().map(|c| c.total),
                reads_epoch: counts.as_ref().map(|c| c.epoch),
                cursor_epoch: cursor.map(|c| c.stream_epoch),
                cursor_seq: cursor.map(|c| c.last_seq),
            });
        }

        let degraded = upstream_error.is_some() || cursors_degraded;
        let upstream_error = if cursors_degraded && upstream_error.is_none() {
            Some("failed to load cursors".to_owned())
        } else {
            upstream_error
        };
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
    pub forwarder_id: String,
    pub reader_ip: String,
    pub local_port_override: Option<u16>,
    pub event_type: Option<crate::db::EventType>,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct UpdatePortRequest {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub local_port_override: Option<u16>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StreamEntry {
    pub forwarder_id: String,
    pub reader_ip: String,
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

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub receiver_id: String,
    pub connection_state: ConnectionState,
    pub local_ok: bool,
    pub streams_count: usize,
}

#[derive(Debug, Serialize)]
pub struct LogsResponse {
    pub entries: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EarliestEpochRequest {
    pub forwarder_id: String,
    pub reader_ip: String,
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

#[derive(Debug, Deserialize)]
struct UpstreamRaceStreamMappingsResponse {
    mappings: Vec<UpstreamRaceStreamMapping>,
}

#[derive(Debug, Deserialize)]
struct UpstreamRaceStreamMapping {
    forwarder_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EventTypeRequest {
    pub event_type: crate::db::EventType,
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
pub struct UpstreamStreamInfo {
    pub stream_id: String,
    pub forwarder_id: String,
    pub reader_ip: String,
    pub display_alias: Option<String>,
    pub stream_epoch: i64,
    pub online: bool,
    #[serde(default)]
    pub reader_connected: bool,
    pub current_epoch_name: Option<String>,
}

/// Normalize a server URL by prepending `ws://` if no scheme is present.
/// Use `wss://` explicitly in the URL for a TLS connection.
pub fn normalize_server_url(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    if trimmed.starts_with("ws://") || trimmed.starts_with("wss://") {
        trimmed.to_owned()
    } else {
        format!("ws://{trimmed}")
    }
}

/// Derive the HTTP base URL from the stored server base URL.
///
/// `ws://host:port`  -> `http://host:port`
/// `wss://host:port` -> `https://host:port`
pub fn http_base_url(base_url: &str) -> Option<String> {
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
pub async fn fetch_server_streams(
    client: &reqwest::Client,
    ws_url: &str,
) -> Result<Vec<UpstreamStreamInfo>, String> {
    let base = http_base_url(ws_url).ok_or_else(|| "cannot parse upstream URL".to_owned())?;
    let url = format!("{base}/api/v1/streams");

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

/// Flat chip_id -> (bib, name) map for a single race.
type FlatChipMap = HashMap<String, (String, String)>;

/// Parse a participants JSON response into a flat chip_id -> (bib, name) map.
fn parse_participants(body: &serde_json::Value) -> FlatChipMap {
    let mut map = FlatChipMap::new();
    if let Some(participants) = body.get("participants").and_then(|v| v.as_array()) {
        for p in participants {
            let bib = match p.get("bib").and_then(|v| v.as_i64()) {
                Some(b) => b.to_string(),
                None => {
                    tracing::debug!("skipping participant without bib field");
                    continue;
                }
            };
            let first = p.get("first_name").and_then(|v| v.as_str()).unwrap_or("");
            let last = p.get("last_name").and_then(|v| v.as_str()).unwrap_or("");
            let name = format!("{first} {last}").trim().to_owned();

            if let Some(chip_ids) = p.get("chip_ids").and_then(|v| v.as_array()) {
                for chip_val in chip_ids {
                    if let Some(chip_id) = chip_val.as_str() {
                        map.insert(chip_id.to_owned(), (bib.clone(), name.clone()));
                    }
                }
            }
        }
    }
    map
}

/// Fetch a flat chip map for a single race from the upstream server.
async fn fetch_race_chips(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    race_id: &str,
) -> Result<FlatChipMap, String> {
    let url = format!("{base}/api/v1/races/{race_id}/participants");
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("fetch participants failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid JSON: {e}"))?;

    Ok(parse_participants(&body))
}

/// Build a per-forwarder chip lookup for Race mode.  All forwarders that
/// sent the receiver a hello for the given race share the same chip map.
pub async fn fetch_chip_lookup_for_race(
    client: &reqwest::Client,
    ws_url: &str,
    token: &str,
    race_id: &str,
    forwarder_ids: &[String],
) -> Result<crate::session::ChipLookup, String> {
    let base = http_base_url(ws_url).ok_or_else(|| "cannot parse upstream URL".to_owned())?;
    let chips = fetch_race_chips(client, &base, token, race_id).await?;
    let mut lookup = crate::session::ChipLookup::new();
    for fwd in forwarder_ids {
        lookup.insert(fwd.clone(), chips.clone());
    }
    Ok(lookup)
}

pub async fn fetch_forwarder_ids_for_race(
    client: &reqwest::Client,
    ws_url: &str,
    token: &str,
    race_id: &str,
) -> Result<Vec<String>, String> {
    let base = http_base_url(ws_url).ok_or_else(|| "cannot parse upstream URL".to_owned())?;
    let url = format!("{base}/api/v1/races/{race_id}/stream-epochs");
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("fetch race stream mappings failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }

    let body: UpstreamRaceStreamMappingsResponse = resp
        .json()
        .await
        .map_err(|e| format!("invalid race stream mappings JSON: {e}"))?;

    let mut forwarder_ids: Vec<String> = body
        .mappings
        .into_iter()
        .map(|mapping| mapping.forwarder_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    forwarder_ids.sort();
    Ok(forwarder_ids)
}

/// Build a per-forwarder chip lookup for Live mode by querying the server's
/// `forwarder_races` assignments.  Only forwarders with an assigned race get
/// entries; reads from unassigned forwarders will not be enriched.
pub async fn fetch_chip_lookup_for_forwarders(
    client: &reqwest::Client,
    ws_url: &str,
    token: &str,
    forwarder_ids: &[String],
) -> Result<crate::session::ChipLookup, String> {
    if forwarder_ids.is_empty() {
        return Ok(crate::session::ChipLookup::new());
    }

    let base = http_base_url(ws_url).ok_or_else(|| "cannot parse upstream URL".to_owned())?;

    // Fetch all forwarder->race assignments in one call.
    let url = format!("{base}/api/v1/forwarder-races");
    let resp = client
        .get(&url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("fetch forwarder-races failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("server returned {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("invalid forwarder-races JSON: {e}"))?;

    // Build forwarder_id -> race_id mapping for our subscribed forwarders.
    let forwarder_set: std::collections::HashSet<&str> =
        forwarder_ids.iter().map(|s| s.as_str()).collect();
    let mut fwd_to_race: HashMap<String, String> = HashMap::new();

    if let Some(assignments) = body.get("assignments").and_then(|v| v.as_array()) {
        for a in assignments {
            let fwd = a.get("forwarder_id").and_then(|v| v.as_str()).unwrap_or("");
            if forwarder_set.contains(fwd)
                && let Some(rid) = a.get("race_id").and_then(|v| v.as_str())
            {
                fwd_to_race.insert(fwd.to_owned(), rid.to_owned());
            }
        }
    }

    // Fetch participants per unique race, caching to avoid duplicate requests.
    let mut race_chips: HashMap<String, FlatChipMap> = HashMap::new();
    for race_id in fwd_to_race.values() {
        if !race_chips.contains_key(race_id) {
            let url = format!("{base}/api/v1/races/{race_id}/participants");
            let chips = match client.get(&url).bearer_auth(token).send().await {
                Ok(r) if r.status().is_success() => r
                    .json::<serde_json::Value>()
                    .await
                    .map(|b| parse_participants(&b))
                    .unwrap_or_default(),
                _ => FlatChipMap::new(),
            };
            race_chips.insert(race_id.clone(), chips);
        }
    }

    // Build the per-forwarder lookup.
    let mut lookup = crate::session::ChipLookup::new();
    for (fwd, race_id) in &fwd_to_race {
        if let Some(chips) = race_chips.get(race_id) {
            lookup.insert(fwd.clone(), chips.clone());
        }
    }

    Ok(lookup)
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
    let url = normalize_server_url(&body.server_url);

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
            *state.upstream_url.write().await = Some(url);
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
    match db.save_earliest_epoch(&body.forwarder_id, &body.reader_ip, body.earliest_epoch) {
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

// ---------------------------------------------------------------------------
// Race management (via WS proxy session)
// ---------------------------------------------------------------------------

pub async fn get_races(state: &AppState) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyRacesListRequest(
        rt_protocol::ReceiverProxyRacesListRequest {
            request_id: generate_request_id(),
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyRacesListResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            Ok(serde_json::json!({ "races": r.races }))
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn create_race(
    state: &AppState,
    name: String,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyRaceCreateRequest(
        rt_protocol::ReceiverProxyRaceCreateRequest {
            request_id: generate_request_id(),
            name,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyRaceCreateResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            match r.race {
                Some(race) => Ok(serde_json::json!(race)),
                None => Err(ReceiverError::UpstreamError(
                    "server returned success but no race data".to_owned(),
                )),
            }
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn delete_race(state: &AppState, race_id: String) -> Result<(), ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyRaceDeleteRequest(
        rt_protocol::ReceiverProxyRaceDeleteRequest {
            request_id: generate_request_id(),
            race_id,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyRaceDeleteResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            Ok(())
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn get_participants(
    state: &AppState,
    race_id: String,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyParticipantsGetRequest(
        rt_protocol::ReceiverProxyParticipantsGetRequest {
            request_id: generate_request_id(),
            race_id,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyParticipantsGetResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            Ok(serde_json::json!({
                "participants": r.participants,
                "chips_without_participant": r.chips_without_participant,
            }))
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn upload_race_file(
    state: &AppState,
    race_id: String,
    upload_type: String,
    file_data: String,
    file_name: String,
) -> Result<serde_json::Value, ReceiverError> {
    let upload_type_enum = match upload_type.as_str() {
        "participants" => rt_protocol::UploadType::Participants,
        "chips" => rt_protocol::UploadType::Chips,
        other => {
            return Err(ReceiverError::BadRequest(format!(
                "invalid upload_type: {other}, expected 'participants' or 'chips'"
            )));
        }
    };
    let msg = rt_protocol::WsMessage::ReceiverProxyFileUploadRequest(
        rt_protocol::ReceiverProxyFileUploadRequest {
            request_id: generate_request_id(),
            race_id,
            upload_type: upload_type_enum,
            file_data,
            file_name,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyFileUploadResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            Ok(serde_json::json!({ "imported": r.imported }))
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

// ---------------------------------------------------------------------------
// Forwarder list (via HTTP to server) + proxy commands (via WS session:
// config, restart, device control, race assignment) + shared proxy helpers
// ---------------------------------------------------------------------------

/// Generate a process-unique request ID for WS proxy commands.
fn generate_request_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("req-{ts}-{seq}")
}

pub async fn get_forwarders(state: &AppState) -> Result<serde_json::Value, ReceiverError> {
    let profile = {
        let db = state.db.lock().await;
        match db.load_profile() {
            Ok(Some(p)) => p,
            Ok(None) => return Err(ReceiverError::NotFound("no profile".to_owned())),
            Err(e) => return Err(ReceiverError::Internal(e.to_string())),
        }
    };

    let Some(base) = http_base_url(&profile.server_url) else {
        return Err(ReceiverError::BadRequest("invalid upstream URL".to_owned()));
    };
    let url = format!("{base}/api/v1/forwarders");

    let response = match state
        .http_client
        .get(&url)
        .bearer_auth(profile.token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Err(ReceiverError::UpstreamError(format!("fetch failed: {e}")));
        }
    };

    if !response.status().is_success() {
        return Err(ReceiverError::UpstreamError(format!(
            "server returned {}",
            response.status()
        )));
    }

    match response.json::<serde_json::Value>().await {
        Ok(body) => Ok(body),
        Err(e) => Err(ReceiverError::UpstreamError(format!(
            "invalid JSON from upstream: {e}"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Announcer proxy (WS → server)
// ---------------------------------------------------------------------------

pub async fn get_server_streams(state: &AppState) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyStreamsListRequest(
        rt_protocol::ReceiverProxyStreamsListRequest {
            request_id: generate_request_id(),
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyStreamsListResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            // Wrap in {"streams": [...]} to match the HTTP API shape expected by the frontend
            let streams_json = serde_json::to_value(&r.streams).map_err(|e| {
                ReceiverError::UpstreamError(format!("failed to serialize streams: {e}"))
            })?;
            Ok(serde_json::json!({ "streams": streams_json }))
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn get_announcer_config(state: &AppState) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyAnnouncerConfigGetRequest(
        rt_protocol::ReceiverProxyAnnouncerConfigGetRequest {
            request_id: generate_request_id(),
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyAnnouncerConfigResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            Ok(r.config)
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn put_announcer_config(
    state: &AppState,
    body: serde_json::Value,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyAnnouncerConfigSetRequest(
        rt_protocol::ReceiverProxyAnnouncerConfigSetRequest {
            request_id: generate_request_id(),
            payload: body,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyAnnouncerConfigResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            Ok(r.config)
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn reset_announcer(state: &AppState) -> Result<(), ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyAnnouncerResetRequest(
        rt_protocol::ReceiverProxyAnnouncerResetRequest {
            request_id: generate_request_id(),
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyAnnouncerResetResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            Ok(())
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn get_forwarder_race(
    state: &AppState,
    forwarder_id: String,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyForwarderRaceGetRequest(
        rt_protocol::ReceiverProxyForwarderRaceGetRequest {
            request_id: generate_request_id(),
            forwarder_id: forwarder_id.clone(),
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyForwarderRaceGetResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            Ok(serde_json::json!({
                "forwarder_id": r.forwarder_id,
                "race_id": r.race_id,
            }))
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn set_forwarder_race(
    state: &AppState,
    forwarder_id: String,
    race_id: Option<String>,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyForwarderRaceSetRequest(
        rt_protocol::ReceiverProxyForwarderRaceSetRequest {
            request_id: generate_request_id(),
            forwarder_id: forwarder_id.clone(),
            race_id,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyForwarderRaceSetResponse(r) => {
            if !r.ok {
                return Err(ReceiverError::UpstreamError(
                    r.error.unwrap_or_else(|| "unknown error".to_owned()),
                ));
            }
            Ok(serde_json::json!({
                "forwarder_id": r.forwarder_id,
                "race_id": r.race_id,
            }))
        }
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

/// Send a WS command through the active session and wait for a response.
///
/// Uses a 15s timeout. For forwarder proxy requests, the server applies a 10s
/// `PROXY_TIMEOUT` to both send and reply phases independently, so the server's
/// total timeout can reach ~20s in degenerate cases and the local timeout may
/// fire first under backpressure. Non-forwarder proxy requests (announcer,
/// stream-list, race management) are handled server-side with no explicit
/// timeout beyond the database query time.
async fn send_ws_command(
    state: &AppState,
    message: rt_protocol::WsMessage,
) -> Result<rt_protocol::WsMessage, ReceiverError> {
    let tx = {
        let guard = state.ws_cmd_tx.read().await;
        guard
            .clone()
            .ok_or_else(|| ReceiverError::NotConnected("no active WS session".to_owned()))?
    };

    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = crate::session::WsCommand::new(message, reply_tx).map_err(|_| {
        ReceiverError::Internal("send_ws_command called with non-proxy message".to_owned())
    })?;

    tx.send(cmd)
        .await
        .map_err(|_| ReceiverError::NotConnected("WS session closed".to_owned()))?;

    let reply = tokio::time::timeout(std::time::Duration::from_secs(15), reply_rx)
        .await
        .map_err(|_| ReceiverError::UpstreamError("upstream response timeout (15s)".to_owned()))?
        .map_err(|_| ReceiverError::UpstreamError("session closed before reply".to_owned()))?;

    // Surface structured errors from the session loop (e.g. WS_SEND_FAILED,
    // SERIALIZE_ERROR, SESSION_CLOSED) instead of falling through to the
    // caller's catch-all "unexpected response type" branch.
    if let rt_protocol::WsMessage::Error(err) = reply {
        return if err.retryable {
            Err(ReceiverError::NotConnected(err.message))
        } else {
            Err(ReceiverError::UpstreamError(err.message))
        };
    }

    Ok(reply)
}

pub async fn get_forwarder_config(
    state: &AppState,
    forwarder_id: String,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyConfigGetRequest(
        rt_protocol::ReceiverProxyConfigGetRequest {
            request_id: generate_request_id(),
            forwarder_id,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyConfigGetResponse(r) => Ok(serde_json::json!({
            "ok": r.ok,
            "error": r.error,
            "config": r.config,
            "restart_needed": r.restart_needed,
        })),
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn set_forwarder_config(
    state: &AppState,
    forwarder_id: String,
    section: String,
    data: serde_json::Value,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyConfigSetRequest(
        rt_protocol::ReceiverProxyConfigSetRequest {
            request_id: generate_request_id(),
            forwarder_id,
            section,
            payload: data,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyConfigSetResponse(r) => Ok(serde_json::json!({
            "ok": r.ok,
            "error": r.error,
            "restart_needed": r.restart_needed,
        })),
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn restart_forwarder_service(
    state: &AppState,
    forwarder_id: String,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyRestartRequest(
        rt_protocol::ReceiverProxyRestartRequest {
            request_id: generate_request_id(),
            forwarder_id,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyControlResponse(r) => Ok(serde_json::json!({
            "ok": r.ok,
            "error": r.error,
        })),
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn restart_forwarder_device(
    state: &AppState,
    forwarder_id: String,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyDeviceControlRequest(
        rt_protocol::ReceiverProxyDeviceControlRequest {
            request_id: generate_request_id(),
            forwarder_id,
            action: rt_protocol::DeviceControlAction::RestartDevice,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyControlResponse(r) => Ok(serde_json::json!({
            "ok": r.ok,
            "error": r.error,
        })),
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn shutdown_forwarder_device(
    state: &AppState,
    forwarder_id: String,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyDeviceControlRequest(
        rt_protocol::ReceiverProxyDeviceControlRequest {
            request_id: generate_request_id(),
            forwarder_id,
            action: rt_protocol::DeviceControlAction::ShutdownDevice,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyControlResponse(r) => Ok(serde_json::json!({
            "ok": r.ok,
            "error": r.error,
        })),
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}

pub async fn get_replay_target_epochs(
    state: &AppState,
    forwarder_id: String,
    reader_ip: String,
) -> Result<ReplayTargetEpochsResponse, ReceiverError> {
    let profile = {
        let db = state.db.lock().await;
        match db.load_profile() {
            Ok(Some(p)) => p,
            Ok(None) => return Err(ReceiverError::NotFound("no profile".to_owned())),
            Err(e) => return Err(ReceiverError::Internal(e.to_string())),
        }
    };

    let Some(base) = http_base_url(&profile.server_url) else {
        return Err(ReceiverError::BadRequest("invalid upstream URL".to_owned()));
    };

    let streams_url = format!("{base}/api/v1/streams");
    let streams_response = match state
        .http_client
        .get(&streams_url)
        .bearer_auth(&profile.token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Err(ReceiverError::UpstreamError(format!("fetch failed: {e}")));
        }
    };
    if !streams_response.status().is_success() {
        return Err(ReceiverError::UpstreamError(format!(
            "upstream streams returned {}",
            streams_response.status()
        )));
    }
    let upstream_streams = match streams_response.json::<ServerStreamsResponse>().await {
        Ok(body) => body.streams,
        Err(e) => {
            return Err(ReceiverError::UpstreamError(format!(
                "invalid streams JSON from upstream: {e}"
            )));
        }
    };
    let Some(stream) = upstream_streams
        .iter()
        .find(|stream| stream.forwarder_id == forwarder_id && stream.reader_ip == reader_ip)
    else {
        return Err(ReceiverError::NotFound("stream not found".to_owned()));
    };

    let epochs_url = format!("{base}/api/v1/streams/{}/epochs", stream.stream_id);
    let epochs_response = match state
        .http_client
        .get(&epochs_url)
        .bearer_auth(&profile.token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Err(ReceiverError::UpstreamError(format!("fetch failed: {e}")));
        }
    };
    if !epochs_response.status().is_success() {
        return Err(ReceiverError::UpstreamError(format!(
            "upstream epochs returned {}",
            epochs_response.status()
        )));
    }
    let upstream_epochs = match epochs_response
        .json::<Vec<UpstreamStreamEpochOption>>()
        .await
    {
        Ok(body) => body,
        Err(e) => {
            return Err(ReceiverError::UpstreamError(format!(
                "invalid epochs JSON from upstream: {e}"
            )));
        }
    };

    let races_url = format!("{base}/api/v1/races");
    let races_response = match state
        .http_client
        .get(&races_url)
        .bearer_auth(&profile.token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return Err(ReceiverError::UpstreamError(format!("fetch failed: {e}")));
        }
    };
    if !races_response.status().is_success() {
        return Err(ReceiverError::UpstreamError(format!(
            "upstream races returned {}",
            races_response.status()
        )));
    }
    let races = match races_response.json::<UpstreamRacesResponse>().await {
        Ok(body) => body.races,
        Err(e) => {
            return Err(ReceiverError::UpstreamError(format!(
                "invalid races JSON from upstream: {e}"
            )));
        }
    };

    let race_mapping_fetches = races.iter().map(|race| {
        let client = state.http_client.clone();
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
                    return Err(format!("fetch failed: {e}"));
                }
            };
            if !mappings_response.status().is_success() {
                return Err(format!(
                    "upstream stream-epochs for race {} returned {}",
                    race_id,
                    mappings_response.status()
                ));
            }
            let mappings = match mappings_response
                .json::<UpstreamRaceEpochMappingsResponse>()
                .await
            {
                Ok(body) => body.mappings,
                Err(e) => {
                    return Err(format!("invalid stream-epochs JSON from upstream: {e}"));
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
            Err(message) => return Err(ReceiverError::UpstreamError(message)),
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

    Ok(ReplayTargetEpochsResponse { epochs })
}

pub async fn put_subscriptions(
    state: &AppState,
    body: SubscriptionsBody,
) -> Result<(), ReceiverError> {
    let mut seen = std::collections::HashSet::new();
    for s in &body.subscriptions {
        if !seen.insert((s.forwarder_id.clone(), s.reader_ip.clone())) {
            return Err(ReceiverError::BadRequest(
                "duplicate subscriptions".to_owned(),
            ));
        }
    }

    let subs: Vec<Subscription> = body
        .subscriptions
        .into_iter()
        .map(|s| Subscription {
            forwarder_id: s.forwarder_id,
            reader_ip: s.reader_ip,
            local_port_override: s.local_port_override,
            event_type: s.event_type.unwrap_or(crate::db::EventType::Finish),
        })
        .collect();
    let mut db = state.db.lock().await;
    match db.replace_subscriptions(&subs) {
        Ok(()) => {
            drop(db);
            let conn_for_status = state.connection_state.borrow().clone();
            let db = state.db.lock().await;
            let streams_count = db.load_subscriptions().map(|s| s.len()).unwrap_or(0);
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
    match db.load_subscriptions() {
        Ok(subscriptions) => Ok(SubscriptionsBody {
            subscriptions: subscriptions
                .into_iter()
                .map(|s| SubscriptionRequest {
                    forwarder_id: s.forwarder_id,
                    reader_ip: s.reader_ip,
                    local_port_override: s.local_port_override,
                    event_type: Some(s.event_type),
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
    let streams_count = db.load_subscriptions().map(|s| s.len()).unwrap_or(0);
    let local_ok = state.db_integrity_ok;
    StatusResponse {
        receiver_id,
        connection_state: conn,
        local_ok,
        streams_count,
    }
}

pub async fn get_logs(state: &AppState) -> LogsResponse {
    let entries = state.logger.entries();
    LogsResponse { entries }
}

pub fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

pub async fn connect(state: &AppState) {
    state.request_connect().await;
}

pub async fn disconnect(state: &AppState) {
    let current = state.connection_state.borrow().clone();
    if current == ConnectionState::Disconnected {
        return;
    }
    state
        .set_connection_state(ConnectionState::Disconnecting)
        .await;
    state.request_disconnect_shutdown();
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
    forwarder_id: &str,
    reader_ip: &str,
    body: EventTypeRequest,
) -> Result<(), ReceiverError> {
    let db = state.db.lock().await;
    match db.update_subscription_event_type(forwarder_id, reader_ip, body.event_type) {
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
    match db.delete_cursor(&body.forwarder_id, &body.reader_ip) {
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
    match db.delete_earliest_epoch(&body.forwarder_id, &body.reader_ip) {
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
            let streams_count = db.load_subscriptions().map(|s| s.len()).unwrap_or(0);
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
            *state.upstream_url.write().await = None;
            *state.receiver_id.write().await = String::new();
            state.emit_streams_snapshot().await;
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
            *state.upstream_url.write().await = None;
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
    match db.update_subscription_port(
        &body.forwarder_id,
        &body.reader_ip,
        body.local_port_override,
    ) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ReceiverError::NotFound("subscription not found".to_owned())),
        Err(e) => Err(ReceiverError::Internal(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use axum::routing::get;
    use axum::{Json, Router};
    use serde_json::json;
    use tokio::net::TcpListener;

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
    fn normalize_server_url_defaults_ws_scheme() {
        assert_eq!(
            normalize_server_url("127.0.0.1:4000/"),
            "ws://127.0.0.1:4000".to_owned()
        );
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

    async fn run_test_server(router: Router) -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        addr
    }

    #[tokio::test]
    async fn fetch_forwarder_ids_for_race_deduplicates_mapped_forwarders() {
        let race_id = "11111111-1111-1111-1111-111111111111";
        let router = Router::new().route(
            &format!("/api/v1/races/{race_id}/stream-epochs"),
            get(move || async move {
                Json(json!({
                    "mappings": [
                        {
                            "stream_id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa",
                            "forwarder_id": "fwd-a",
                            "reader_ip": "10.0.0.1:10000",
                            "stream_epoch": 1,
                            "race_id": race_id,
                        },
                        {
                            "stream_id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
                            "forwarder_id": "fwd-b",
                            "reader_ip": "10.0.0.2:10000",
                            "stream_epoch": 2,
                            "race_id": race_id,
                        },
                        {
                            "stream_id": "cccccccc-cccc-cccc-cccc-cccccccccccc",
                            "forwarder_id": "fwd-a",
                            "reader_ip": "10.0.0.3:10000",
                            "stream_epoch": 3,
                            "race_id": race_id,
                        }
                    ]
                }))
            }),
        );
        let addr = run_test_server(router).await;

        let ids = fetch_forwarder_ids_for_race(
            &reqwest::Client::new(),
            &format!("ws://{addr}"),
            "test-token",
            race_id,
        )
        .await
        .unwrap();

        assert_eq!(ids, vec!["fwd-a", "fwd-b"]);
    }
}
