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
use rt_updater::UpdateStatus;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
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
    pub logger: Arc<rt_ui_log::UiLogger<ReceiverUiEvent>>,
    pub shutdown_tx: watch::Sender<bool>,
    pub upstream_url: Arc<RwLock<Option<String>>>,
    pub ui_tx: broadcast::Sender<ReceiverUiEvent>,
    pub update_status: Arc<RwLock<UpdateStatus>>,
    pub staged_update_path: Arc<RwLock<Option<PathBuf>>>,
    pub update_mode: Arc<RwLock<rt_updater::UpdateMode>>,
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
        });
        (state, shutdown_rx)
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
        self.logger.log(label);
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
    #[serde(default)]
    pub update_mode: Option<String>,
}

fn default_update_mode() -> String {
    "check-and-download".to_owned()
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
            update_mode: p.update_mode,
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
    let entries = state.logger.entries();
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

async fn get_update_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let status = state.update_status.read().await.clone();
    Json(status).into_response()
}

async fn post_update_apply(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let path = state.staged_update_path.read().await.clone();
    match path {
        Some(path) => {
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
        Ok(c) => RealUpdateChecker { inner: c },
        Err(e) => {
            let status = rt_updater::UpdateStatus::Failed {
                error: e.to_string(),
            };
            *state.update_status.write().await = status.clone();
            return Json(status).into_response();
        }
    };

    let status = run_update_check_with_checker(&state, &checker, update_mode).await;
    Json(status).into_response()
}

trait UpdateCheckClient: Send + Sync {
    fn check<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<UpdateStatus, String>> + Send + 'a>>;

    fn download<'a>(
        &'a self,
        version: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PathBuf, String>> + Send + 'a>>;
}

struct RealUpdateChecker {
    inner: rt_updater::UpdateChecker,
}

impl UpdateCheckClient for RealUpdateChecker {
    fn check<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<UpdateStatus, String>> + Send + 'a>> {
        Box::pin(async move { self.inner.check().await.map_err(|e| e.to_string()) })
    }

    fn download<'a>(
        &'a self,
        version: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<PathBuf, String>> + Send + 'a>> {
        Box::pin(async move {
            self.inner
                .download(version)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

async fn run_update_check_with_checker(
    state: &Arc<AppState>,
    checker: &dyn UpdateCheckClient,
    update_mode: rt_updater::UpdateMode,
) -> UpdateStatus {
    match checker.check().await {
        Ok(UpdateStatus::Available { version }) => {
            *state.update_status.write().await = UpdateStatus::Available {
                version: version.clone(),
            };

            if update_mode == rt_updater::UpdateMode::CheckAndDownload {
                match checker.download(&version).await {
                    Ok(path) => {
                        let status = UpdateStatus::Downloaded {
                            version: version.clone(),
                        };
                        *state.update_status.write().await = status.clone();
                        *state.staged_update_path.write().await = Some(path);
                        let _ = state.ui_tx.send(
                            crate::ui_events::ReceiverUiEvent::UpdateStatusChanged {
                                status: status.clone(),
                            },
                        );
                        status
                    }
                    Err(error) => {
                        let status = UpdateStatus::Failed { error };
                        *state.update_status.write().await = status.clone();
                        let _ = state.ui_tx.send(
                            crate::ui_events::ReceiverUiEvent::UpdateStatusChanged {
                                status: status.clone(),
                            },
                        );
                        status
                    }
                }
            } else {
                let status = UpdateStatus::Available {
                    version: version.clone(),
                };
                let _ = state
                    .ui_tx
                    .send(crate::ui_events::ReceiverUiEvent::UpdateStatusChanged {
                        status: status.clone(),
                    });
                status
            }
        }
        Ok(status) => {
            *state.update_status.write().await = status.clone();
            let _ = state
                .ui_tx
                .send(crate::ui_events::ReceiverUiEvent::UpdateStatusChanged {
                    status: status.clone(),
                });
            status
        }
        Err(error) => {
            let status = UpdateStatus::Failed { error };
            *state.update_status.write().await = status.clone();
            let _ = state
                .ui_tx
                .send(crate::ui_events::ReceiverUiEvent::UpdateStatusChanged {
                    status: status.clone(),
                });
            status
        }
    }
}

async fn run_update_download_with_checker(
    state: &Arc<AppState>,
    checker: &dyn UpdateCheckClient,
) -> Result<UpdateStatus, UpdateStatus> {
    let current = state.update_status.read().await.clone();
    match current {
        UpdateStatus::Available { ref version } => match checker.download(version).await {
            Ok(path) => {
                let status = UpdateStatus::Downloaded {
                    version: version.clone(),
                };
                *state.update_status.write().await = status.clone();
                *state.staged_update_path.write().await = Some(path);
                let _ = state
                    .ui_tx
                    .send(crate::ui_events::ReceiverUiEvent::UpdateStatusChanged {
                        status: status.clone(),
                    });
                Ok(status)
            }
            Err(error) => {
                let status = UpdateStatus::Failed { error };
                *state.update_status.write().await = status.clone();
                Err(status)
            }
        },
        s @ UpdateStatus::Downloaded { .. } => Ok(s),
        other => Err(other),
    }
}

async fn post_update_download(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let checker = match rt_updater::UpdateChecker::new(
        "iwismer",
        "rusty-timer",
        "receiver",
        env!("CARGO_PKG_VERSION"),
    ) {
        Ok(c) => RealUpdateChecker { inner: c },
        Err(e) => {
            let status = rt_updater::UpdateStatus::Failed {
                error: e.to_string(),
            };
            *state.update_status.write().await = status.clone();
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(status)).into_response();
        }
    };

    match run_update_download_with_checker(&state, &checker).await {
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
        .route("/api/v1/streams", get(get_streams))
        .route("/api/v1/subscriptions", put(put_subscriptions))
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/logs", get(get_logs))
        .route("/api/v1/connect", post(post_connect))
        .route("/api/v1/disconnect", post(post_disconnect))
        .route("/api/v1/events", get(crate::sse::receiver_sse))
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
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    struct FakeChecker {
        check_result: Result<UpdateStatus, String>,
        download_result: Result<std::path::PathBuf, String>,
        download_calls: Arc<AtomicUsize>,
    }

    impl UpdateCheckClient for FakeChecker {
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

        let status =
            run_update_check_with_checker(&state, &checker, rt_updater::UpdateMode::CheckOnly)
                .await;

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

        let status = run_update_check_with_checker(
            &state,
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

        let status = run_update_download_with_checker(&state, &checker).await;

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
    async fn update_download_returns_conflict_when_up_to_date() {
        let temp = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&temp.path().join("receiver.sqlite3")).expect("open db");
        let (state, _shutdown_rx) = AppState::new(db);

        let checker = FakeChecker {
            check_result: Ok(UpdateStatus::UpToDate),
            download_result: Ok(std::path::PathBuf::from("/tmp/unused")),
            download_calls: Arc::new(AtomicUsize::new(0)),
        };

        let status = run_update_download_with_checker(&state, &checker).await;
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

        let status = run_update_download_with_checker(&state, &checker).await;
        assert_eq!(
            status,
            Ok(UpdateStatus::Downloaded {
                version: "2.0.0".to_owned()
            })
        );
    }
}
