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

use crate::db::{Db, Subscription};
use axum::routing::{get, post, put};
use axum::{extract::State, http::StatusCode, response::IntoResponse, Json, Router};
use rt_protocol::StreamInfo;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{watch, Mutex, RwLock};
use tracing::{info, warn};

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
}

impl AppState {
    pub fn new(db: Db) -> (Arc<Self>, watch::Receiver<bool>) {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let state = Arc::new(Self {
            db: Arc::new(Mutex::new(db)),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            log_entries: Arc::new(RwLock::new(Vec::new())),
            shutdown_tx,
            upstream_url: Arc::new(RwLock::new(None)),
        });
        (state, shutdown_rx)
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

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
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

/// Derive the HTTP base URL from a WebSocket URL.
///
/// `ws://host:port/ws/v1/receivers`  → `http://host:port`
/// `wss://host:port/ws/v1/receivers` → `https://host:port`
pub(crate) fn http_base_url(ws_url: &str) -> Option<String> {
    let url = reqwest::Url::parse(ws_url).ok()?;
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
    let db = state.db.lock().await;
    match db.save_profile(&body.server_url, &body.token, &body.log_level) {
        Ok(()) => {
            drop(db);
            *state.upstream_url.write().await = Some(body.server_url);
            StatusCode::NO_CONTENT.into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn get_streams(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // 1. Load local subscriptions from SQLite.
    let db = state.db.lock().await;
    let subs = match db.load_subscriptions() {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    drop(db);

    // 2. Build a lookup map of local subscriptions: (forwarder_id, reader_ip) -> &Subscription
    let sub_map: HashMap<(&str, &str), &Subscription> = subs
        .iter()
        .map(|s| ((s.forwarder_id.as_str(), s.reader_ip.as_str()), s))
        .collect();

    // 3. Attempt to fetch server streams if connected.
    let upstream_url = state.upstream_url.read().await.clone();
    let conn_state = state.connection_state.read().await.clone();

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

    // 4. Merge: server streams first, then any local-only subscriptions.
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

    // Append local subscriptions not present in the server list.
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
    Json(StreamsResponse {
        streams,
        degraded,
        upstream_error,
    })
    .into_response()
}

async fn put_subscriptions(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SubscriptionsBody>,
) -> impl IntoResponse {
    let subs: Vec<Subscription> = body
        .subscriptions
        .into_iter()
        .map(|s| Subscription {
            forwarder_id: s.forwarder_id,
            reader_ip: s.reader_ip,
            local_port_override: s.local_port_override,
        })
        .collect();
    let db = state.db.lock().await;
    match db.replace_subscriptions(&subs) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
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
        info!("already connected, no-op");
        return StatusCode::OK.into_response();
    }
    *state.connection_state.write().await = ConnectionState::Connecting;
    info!("connect requested (async)");
    StatusCode::ACCEPTED.into_response()
}

async fn post_disconnect(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let current = state.connection_state.read().await.clone();
    if current == ConnectionState::Disconnected {
        info!("already disconnected, no-op");
        return StatusCode::OK.into_response();
    }
    *state.connection_state.write().await = ConnectionState::Disconnecting;
    let _ = state.shutdown_tx.send(true);
    info!("disconnect requested (async)");
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
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_base_url_ws_with_port() {
        assert_eq!(
            http_base_url("ws://127.0.0.1:8080/ws/v1/receivers"),
            Some("http://127.0.0.1:8080".to_owned())
        );
    }

    #[test]
    fn http_base_url_wss_with_port() {
        assert_eq!(
            http_base_url("wss://server.example.com:8443/ws/v1/receivers"),
            Some("https://server.example.com:8443".to_owned())
        );
    }

    #[test]
    fn http_base_url_wss_no_port() {
        assert_eq!(
            http_base_url("wss://server.example.com/ws/v1/receivers"),
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
}
