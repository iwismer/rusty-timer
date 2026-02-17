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

use std::sync::Arc;
use axum::{Router, Json, extract::State, http::StatusCode, response::IntoResponse};
use axum::routing::{get, put, post};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch, RwLock};
use tracing::info;
use crate::db::{Db, Subscription};

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
// Route handlers
// ---------------------------------------------------------------------------

async fn get_profile(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let db = state.db.lock().await;
    match db.load_profile() {
        Ok(Some(p)) => Json(ProfileResponse { server_url: p.server_url, token: p.token, log_level: p.log_level }).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "no profile").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn put_profile(State(state): State<Arc<AppState>>, Json(body): Json<ProfileRequest>) -> impl IntoResponse {
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
    let db = state.db.lock().await;
    let subs = match db.load_subscriptions() {
        Ok(s) => s,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    drop(db);

    let upstream_url = state.upstream_url.read().await.clone();
    let upstream_error: Option<String> = match upstream_url {
        None => Some("no profile configured".to_owned()),
        Some(_) => {
            let conn = state.connection_state.read().await.clone();
            if conn != ConnectionState::Connected {
                Some(format!("connection state: {conn:?}"))
            } else {
                None
            }
        }
    };

    let streams: Vec<StreamEntry> = subs.iter().map(|s| {
        let port = s.local_port_override
            .or_else(|| crate::ports::default_port(&s.reader_ip));
        StreamEntry {
            forwarder_id: s.forwarder_id.clone(),
            reader_ip: s.reader_ip.clone(),
            subscribed: true,
            local_port: port,
        }
    }).collect();

    let degraded = upstream_error.is_some();
    Json(StreamsResponse { streams, degraded, upstream_error }).into_response()
}

async fn put_subscriptions(State(state): State<Arc<AppState>>, Json(body): Json<SubscriptionsBody>) -> impl IntoResponse {
    let subs: Vec<Subscription> = body.subscriptions.into_iter().map(|s| Subscription {
        forwarder_id: s.forwarder_id,
        reader_ip: s.reader_ip,
        local_port_override: s.local_port_override,
    }).collect();
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
    Json(StatusResponse { connection_state: conn, local_ok, streams_count }).into_response()
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
        .route("/api/v1/profile",       get(get_profile).put(put_profile))
        .route("/api/v1/streams",       get(get_streams))
        .route("/api/v1/subscriptions", put(put_subscriptions))
        .route("/api/v1/status",        get(get_status))
        .route("/api/v1/logs",          get(get_logs))
        .route("/api/v1/connect",       post(post_connect))
        .route("/api/v1/disconnect",    post(post_disconnect))
        .with_state(state)
}
