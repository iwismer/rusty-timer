//! Localhost control API for the receiver.
//!
//! Binds to 127.0.0.1:9090 (or a caller-supplied address for tests).
//! Routes:
//!   GET  /api/v1/profile        - read current profile
//!   PUT  /api/v1/profile        - update profile
//!   GET  /api/v1/streams        - list streams (merges server + local subs)
//!   PUT  /api/v1/subscriptions  - replace subscription list
//!   GET  /api/v1/status         - runtime status
//!   GET  /api/v1/logs           - recent log entries
//!   POST /api/v1/connect        - initiate WS connection (async, 202)
//!   POST /api/v1/disconnect     - close WS connection (async, 202)
//!   GET  /api/v1/events         - SSE stream of receiver UI events

use crate::db::{Db, Subscription};
use crate::ui_events::ReceiverUiEvent;
use axum::routing::{get, post, put};
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json, Router};
use rt_protocol::StreamInfo;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{broadcast, watch, Mutex, RwLock};
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

pub struct AppState {
    pub db: Arc<Mutex<Db>>,
    pub connection_state: Arc<RwLock<ConnectionState>>,
    pub log_entries: Arc<RwLock<Vec<String>>>,
    pub shutdown_tx: watch::Sender<bool>,
    pub upstream_url: Arc<RwLock<Option<String>>>,
    pub ui_tx: broadcast::Sender<ReceiverUiEvent>,
}

impl AppState {
    pub fn new(db: Db) -> (Arc<Self>, watch::Receiver<bool>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let (ui_tx, _) = broadcast::channel(256);
        let state = Arc::new(Self {
            db: Arc::new(Mutex::new(db)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            log_entries: Arc::new(RwLock::new(Vec::new())),
            shutdown_tx,
            upstream_url: Arc::new(RwLock::new(None)),
            ui_tx,
        });
        (state, shutdown_rx)
    }

    /// Cap for the in-memory log ring buffer.
    const MAX_LOG_ENTRIES: usize = 500;

    /// Append a timestamped log entry and broadcast it to SSE clients.
    pub async fn emit_log(&self, message: String) {
        let entry = format!(
            "{} {}",
            chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
            message
        );
        {
            let mut entries = self.log_entries.write().await;
            entries.push(entry.clone());
            if entries.len() > Self::MAX_LOG_ENTRIES {
                let excess = entries.len() - Self::MAX_LOG_ENTRIES;
                entries.drain(..excess);
            }
        }
        let _ = self.ui_tx.send(ReceiverUiEvent::LogEntry { entry });
    }

    /// Update connection state, broadcast status change, and emit a log entry.
    pub async fn set_connection_state(&self, new_state: ConnectionState) {
        *self.connection_state.write().await = new_state.clone();
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
        self.emit_log(label.to_owned()).await;
    }

    /// Build the merged streams response from local subscriptions and upstream server.
    pub async fn build_streams_response(&self) -> StreamsResponse {
        let db = self.db.lock().await;
        let subs = match db.load_subscriptions() {
            Ok(s) => s,
            Err(_) => {
                return StreamsResponse {
                    streams: vec![],
                    degraded: true,
                    upstream_error: Some("failed to load subscriptions".to_owned()),
                }
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
                streams.push(StreamEntry {
                    forwarder_id: si.forwarder_id.clone(),
                    reader_ip: si.reader_ip.clone(),
                    subscribed: local.is_some(),
                    local_port: port,
                    online: Some(si.online),
                    display_alias: si.display_alias.clone(),
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
            streams.push(StreamEntry {
                forwarder_id: sub.forwarder_id.clone(),
                reader_ip: sub.reader_ip.clone(),
                subscribed: true,
                local_port: port,
                online: None,
                display_alias: None,
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
    pub log_level: String,
}

#[derive(Debug, Serialize)]
pub struct ProfileResponse {
    pub server_url: String,
    pub token: String,
    pub log_level: String,
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

// ---------------------------------------------------------------------------
// Server stream fetching helpers
// ---------------------------------------------------------------------------

/// Response shape from the server's `GET /api/v1/streams`.
#[derive(Debug, Deserialize)]
struct ServerStreamsResponse {
    streams: Vec<StreamInfo>,
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
async fn fetch_server_streams(ws_url: &str) -> Result<Vec<StreamInfo>, String> {
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
            log_level: p.log_level,
        })
        .into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no profile").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn put_profile(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ProfileRequest>,
) -> impl IntoResponse {
    let url = normalize_server_url(&body.server_url);
    let db = state.db.lock().await;
    match db.save_profile(&url, &body.token, &body.log_level) {
        Ok(()) => {
            drop(db);
            *state.upstream_url.write().await = Some(url);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_streams(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    Json(state.build_streams_response().await).into_response()
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
    let entries = state.log_entries.read().await.clone();
    Json(LogsResponse { entries }).into_response()
}

async fn post_connect(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let current = state.connection_state.read().await.clone();
    if current == ConnectionState::Connected {
        return StatusCode::OK.into_response();
    }
    state
        .set_connection_state(ConnectionState::Connecting)
        .await;
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

// ---------------------------------------------------------------------------
// Router builder
// ---------------------------------------------------------------------------

pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/v1/profile", get(get_profile).put(put_profile))
        .route("/api/v1/streams", get(get_streams))
        .route("/api/v1/subscriptions", put(put_subscriptions))
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/logs", get(get_logs))
        .route("/api/v1/connect", post(post_connect))
        .route("/api/v1/disconnect", post(post_disconnect))
        .route("/api/v1/events", get(crate::sse::receiver_sse))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
