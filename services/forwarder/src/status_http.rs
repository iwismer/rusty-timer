//! Local status HTTP server for Task 8.
//!
//! Provides:
//! - `GET /`              — read-only HTML status page
//! - `GET /healthz`       — always 200 OK (process is running)
//! - `GET /readyz`        — 200 when local subsystems ready, 503 otherwise
//! - `POST /api/v1/streams/{reader_ip}/reset-epoch`
//!   — bump stream epoch; 200 on success, 404 if unknown
//! - `GET /update/status`    — current rt-updater status as JSON
//! - `POST /update/apply`    — apply a staged update and exit
//!
//! # Readiness contract
//! `/readyz` reflects local prerequisites only (config + SQLite + worker loops).
//! Uplink connectivity does NOT affect readiness.
//!
//! # Security
//! No authentication in v1. Status page is read-only.

use crate::storage::journal::Journal;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use rt_updater::UpdateStatus;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Public config
// ---------------------------------------------------------------------------

/// Configuration for the status HTTP server.
#[derive(Debug, Clone)]
pub struct StatusConfig {
    /// Bind address, e.g. `"0.0.0.0:8080"`.
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

/// Per-reader status tracked in memory.
#[derive(Debug, Clone)]
pub struct ReaderStatus {
    pub state: ReaderConnectionState,
    pub last_seen: Option<Instant>,
    pub reads_since_restart: u64,
    pub reads_total: i64,
}

/// Tracks local subsystem readiness for the `/readyz` endpoint.
///
/// Ready = config loaded + journal open + worker tasks started.
/// Uplink connectivity is explicitly excluded from readiness.
#[derive(Debug, Clone)]
pub struct SubsystemStatus {
    ready: bool,
    reason: Option<String>,
    /// Uplink state is tracked for the status page but does NOT affect readiness.
    uplink_connected: bool,
    forwarder_id: String,
    local_ip: Option<String>,
    readers: HashMap<String, ReaderStatus>,
    update_status: UpdateStatus,
    staged_update_path: Option<std::path::PathBuf>,
    /// Set to `true` when config is saved and the forwarder needs a restart to apply changes.
    restart_needed: bool,
}

impl SubsystemStatus {
    /// Create a fully-ready subsystem status.
    pub fn ready() -> Self {
        SubsystemStatus {
            ready: true,
            reason: None,
            uplink_connected: false,
            forwarder_id: String::new(),
            local_ip: None,
            readers: HashMap::new(),
            update_status: UpdateStatus::UpToDate,
            staged_update_path: None,
            restart_needed: false,
        }
    }

    /// Create a not-ready subsystem status with a reason.
    pub fn not_ready(reason: String) -> Self {
        SubsystemStatus {
            ready: false,
            reason: Some(reason),
            uplink_connected: false,
            forwarder_id: String::new(),
            local_ip: None,
            readers: HashMap::new(),
            update_status: UpdateStatus::UpToDate,
            staged_update_path: None,
            restart_needed: false,
        }
    }

    /// Set the uplink connection state (does NOT affect `/readyz` result).
    pub fn set_uplink_connected(&mut self, connected: bool) {
        self.uplink_connected = connected;
    }

    /// Return true if all local subsystems are ready.
    pub fn is_ready(&self) -> bool {
        self.ready
    }

    /// Return the uplink connection state.
    pub fn uplink_connected(&self) -> bool {
        self.uplink_connected
    }

    /// Return whether a restart is needed to apply saved config changes.
    pub fn restart_needed(&self) -> bool {
        self.restart_needed
    }

    /// Mark that a restart is needed to apply saved config changes.
    pub fn set_restart_needed(&mut self) {
        self.restart_needed = true;
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
}

/// Holds the config file path and a write lock for read-modify-write operations.
pub struct ConfigState {
    pub path: std::path::PathBuf,
    write_lock: Mutex<()>,
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
}

impl<J: JournalAccess + Send + 'static> Clone for AppState<J> {
    fn clone(&self) -> Self {
        Self {
            subsystem: self.subsystem.clone(),
            journal: self.journal.clone(),
            version: self.version.clone(),
            config_state: self.config_state.clone(),
        }
    }
}

impl StatusServer {
    /// Return the bound listen address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Mark all local subsystems as ready.
    pub async fn set_ready(&self) {
        let mut ss = self.subsystem.lock().await;
        ss.ready = true;
        ss.reason = None;
    }

    /// Mark that a restart is needed to apply saved config changes.
    pub async fn set_restart_needed(&self) {
        self.subsystem.lock().await.set_restart_needed();
    }

    /// Return whether a restart is needed to apply saved config changes.
    pub async fn restart_needed(&self) -> bool {
        self.subsystem.lock().await.restart_needed()
    }

    /// Update the uplink connection state (does not affect readiness).
    pub async fn set_uplink_connected(&self, connected: bool) {
        self.subsystem.lock().await.set_uplink_connected(connected);
    }

    /// Set the forwarder ID (call once at startup).
    pub async fn set_forwarder_id(&self, id: &str) {
        self.subsystem.lock().await.forwarder_id = id.to_owned();
    }

    /// Set the detected local IP (call once at startup).
    pub async fn set_local_ip(&self, ip: Option<String>) {
        self.subsystem.lock().await.local_ip = ip;
    }

    /// Update the current rt-updater status (shown on `/update/status`).
    pub async fn set_update_status(&self, status: UpdateStatus) {
        self.subsystem.lock().await.update_status = status;
    }

    /// Record the filesystem path of a downloaded update artifact ready to apply.
    pub async fn set_staged_update_path(&self, path: std::path::PathBuf) {
        self.subsystem.lock().await.staged_update_path = Some(path);
    }

    /// Pre-populate all configured reader IPs as Disconnected.
    pub async fn init_readers(&self, reader_ips: &[String]) {
        let mut ss = self.subsystem.lock().await;
        for ip in reader_ips {
            ss.readers.entry(ip.clone()).or_insert(ReaderStatus {
                state: ReaderConnectionState::Disconnected,
                last_seen: None,
                reads_since_restart: 0,
                reads_total: 0,
            });
        }
    }

    /// Seed a reader's total historical count from durable journal state.
    pub async fn set_reader_total(&self, reader_ip: &str, total: i64) {
        let mut ss = self.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(reader_ip) {
            r.reads_total = total;
        }
    }

    /// Update a reader's connection state.
    pub async fn update_reader_state(&self, reader_ip: &str, state: ReaderConnectionState) {
        let mut ss = self.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(reader_ip) {
            r.state = state;
        }
    }

    /// Record a successful chip read for a reader.
    pub async fn record_read(&self, reader_ip: &str) {
        let mut ss = self.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(reader_ip) {
            r.reads_since_restart += 1;
            r.reads_total += 1;
            r.last_seen = Some(Instant::now());
        }
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

        let subsystem = Arc::new(Mutex::new(subsystem));
        let state = AppState {
            subsystem: subsystem.clone(),
            journal,
            version: Arc::new(cfg.forwarder_version),
            config_state: None,
        };

        let app = build_router(state);
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                eprintln!("status HTTP server error: {}", err);
            }
        });

        Ok(StatusServer {
            local_addr,
            subsystem,
        })
    }

    /// Start the status HTTP server with config editing support.
    pub async fn start_with_config<J: JournalAccess + Send + 'static>(
        cfg: StatusConfig,
        subsystem: SubsystemStatus,
        journal: Arc<Mutex<J>>,
        config_state: ConfigState,
    ) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind(&cfg.bind).await?;
        let local_addr = listener.local_addr()?;

        let subsystem = Arc::new(Mutex::new(subsystem));
        let state = AppState {
            subsystem: subsystem.clone(),
            journal,
            version: Arc::new(cfg.forwarder_version),
            config_state: Some(Arc::new(config_state)),
        };

        let app = build_router(state);
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, app).await {
                eprintln!("status HTTP server error: {}", err);
            }
        });

        Ok(StatusServer {
            local_addr,
            subsystem,
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

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

fn format_last_seen(instant: Option<Instant>) -> String {
    match instant {
        None => "never".to_owned(),
        Some(t) => {
            let elapsed = t.elapsed().as_secs();
            if elapsed < 60 {
                format!("{}s ago", elapsed)
            } else if elapsed < 3600 {
                format!("{}m ago", elapsed / 60)
            } else {
                format!("{}h ago", elapsed / 3600)
            }
        }
    }
}

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
    mutate: impl FnOnce(&mut crate::config::RawConfig) -> Result<(), String>,
) -> Result<(), (u16, String)> {
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

    write_atomic(&config_state.path, &new_toml).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("File write error: {}", e)})
                .to_string(),
        )
    })?;

    subsystem.lock().await.set_restart_needed();
    Ok(())
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

fn require_non_empty_trimmed(field: &str, value: Option<String>) -> Result<String, String> {
    let raw = value.ok_or_else(|| format!("{} is required", field))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("{} must not be empty", field));
    }
    Ok(trimmed.to_owned())
}

fn validate_base_url(base_url: &str) -> Result<(), String> {
    let uri: Uri = base_url
        .parse()
        .map_err(|_| "base_url must be a valid absolute URL".to_owned())?;
    let scheme = uri
        .scheme_str()
        .ok_or_else(|| "base_url must include scheme".to_owned())?;
    if scheme != "http" && scheme != "https" {
        return Err("base_url scheme must be http or https".to_owned());
    }
    if uri.authority().is_none() {
        return Err("base_url must include host".to_owned());
    }
    Ok(())
}

fn validate_token_file(token_file: &str) -> Result<(), String> {
    if token_file.contains('\n') || token_file.contains('\r') {
        return Err("token_file must be a single-line path".to_owned());
    }
    Ok(())
}

fn config_not_available() -> Response {
    text_response(StatusCode::NOT_FOUND, "Config editing not available")
}

fn get_config_state<J: JournalAccess + Send + 'static>(
    state: &AppState<J>,
) -> Option<Arc<ConfigState>> {
    state.config_state.clone()
}

fn build_router<J: JournalAccess + Send + 'static>(state: AppState<J>) -> Router {
    Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/readyz", get(readyz_handler::<J>))
        .route("/", get(status_page_handler::<J>))
        .route(
            "/api/v1/streams/:reader_ip/reset-epoch",
            post(reset_epoch_handler::<J>),
        )
        .route("/update/status", get(update_status_handler::<J>))
        .route("/update/apply", post(update_apply_handler::<J>))
        .route("/config", get(config_page_handler::<J>))
        .route("/api/v1/config", get(config_json_handler::<J>))
        .route(
            "/api/v1/config/general",
            post(post_config_general_handler::<J>),
        )
        .route(
            "/api/v1/config/server",
            post(post_config_server_handler::<J>),
        )
        .route("/api/v1/config/auth", post(post_config_auth_handler::<J>))
        .route(
            "/api/v1/config/journal",
            post(post_config_journal_handler::<J>),
        )
        .route(
            "/api/v1/config/uplink",
            post(post_config_uplink_handler::<J>),
        )
        .route(
            "/api/v1/config/status_http",
            post(post_config_status_http_handler::<J>),
        )
        .route(
            "/api/v1/config/readers",
            post(post_config_readers_handler::<J>),
        )
        .fallback(not_found_handler)
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

async fn status_page_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Html<String> {
    let (ready, uplink_connected, forwarder_id, local_ip, readers, restart_needed) = {
        let ss = state.subsystem.lock().await;
        let mut readers: Vec<_> = ss
            .readers
            .iter()
            .map(|(ip, r)| (ip.clone(), r.clone()))
            .collect();
        readers.sort_by(|a, b| a.0.cmp(&b.0));
        (
            ss.is_ready(),
            ss.uplink_connected(),
            ss.forwarder_id.clone(),
            ss.local_ip.clone(),
            readers,
            ss.restart_needed(),
        )
    };

    let ready_state = if ready { "ready" } else { "not-ready" };
    let ready_class = if ready { "ok" } else { "err" };
    let uplink_state = if uplink_connected {
        "connected"
    } else {
        "disconnected"
    };
    let uplink_class = if uplink_connected { "ok" } else { "err" };
    let local_ip_display = local_ip.as_deref().unwrap_or("unknown");

    let mut reader_rows = String::new();
    for (ip, r) in &readers {
        let (state_text, state_class) = match r.state {
            ReaderConnectionState::Connected => ("connected", "ok"),
            ReaderConnectionState::Connecting => ("connecting", "warn"),
            ReaderConnectionState::Disconnected => ("disconnected", "err"),
        };
        let last_seen = format_last_seen(r.last_seen);
        reader_rows.push_str(&format!(
            "<tr><td>{ip}</td>\
             <td><span class=\"status {sc}\">{st}</span></td>\
             <td>{session}</td>\
             <td>{total}</td>\
             <td>{ls}</td></tr>",
            ip = ip,
            sc = state_class,
            st = state_text,
            session = r.reads_since_restart,
            total = r.reads_total,
            ls = last_seen,
        ));
    }

    let restart_banner = if restart_needed {
        "<div style=\"background:#fff3cd;color:#856404;border:1px solid #ffc107;padding:.75rem 1rem;border-radius:4px;margin-bottom:1rem\">Configuration changed. Restart the forwarder to apply changes.</div>"
    } else {
        ""
    };

    let html = format!(
        "<!DOCTYPE html>\
         <html><head><title>Forwarder Status</title>\
         <style>\
         body{{font-family:system-ui,sans-serif;max-width:600px;margin:2rem auto;padding:0 1rem}}\
         h1{{margin-bottom:.5rem}}\
         h2{{margin-top:1.5rem;margin-bottom:.5rem}}\
         .status{{padding:.25rem .5rem;border-radius:4px;display:inline-block}}\
         .ok{{background:#d4edda;color:#155724}}\
         .warn{{background:#fff3cd;color:#856404}}\
         .err{{background:#f8d7da;color:#721c24}}\
         table{{border-collapse:collapse;width:100%}}\
         th,td{{text-align:left;padding:.4rem .6rem;border-bottom:1px solid #ddd}}\
         th{{font-weight:600}}\
         </style>\
         </head><body>\
         {restart_banner}\
         <h1>Forwarder Status</h1>\
         <p><a href=\"/config\">Configure</a></p>\
         <p>Version: {version}</p>\
         <p>Forwarder ID: <code>{fwd_id}</code></p>\
         <p>Local IP: {local_ip}</p>\
         <p>Readiness: <span class=\"status {rc}\">{rs}</span></p>\
         <p>Uplink: <span class=\"status {uc}\">{us}</span></p>\
         <h2>Readers</h2>\
         <table>\
         <tr><th>Reader IP</th><th>Status</th><th>Reads (session)</th><th>Reads (total)</th><th>Last seen</th></tr>\
         {reader_rows}\
         </table>\
         <script>\
         setTimeout(()=>location.reload(),2000);\
         </script>\
         </body></html>",
        restart_banner = restart_banner,
        version = *state.version,
        fwd_id = forwarder_id,
        local_ip = local_ip_display,
        rs = ready_state,
        rc = ready_class,
        us = uplink_state,
        uc = uplink_class,
        reader_rows = reader_rows,
    );

    Html(html)
}

async fn reset_epoch_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    Path(reader_ip): Path<String>,
) -> Response {
    // Keep prior behavior for malformed percent-encoding style stream keys.
    if reader_ip.contains('%') {
        return text_response(
            StatusCode::BAD_REQUEST,
            "invalid percent-encoding in stream key",
        );
    }

    let result = state.journal.lock().await.reset_epoch(&reader_ip);
    match result {
        Ok(new_epoch) => {
            let body = format!("{{\"new_epoch\":{}}}", new_epoch);
            json_response(StatusCode::OK, body)
        }
        Err(EpochResetError::NotFound) => text_response(StatusCode::NOT_FOUND, "stream not found"),
        Err(EpochResetError::Storage(e)) => text_response(StatusCode::INTERNAL_SERVER_ERROR, e),
    }
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
            let sub = state.subsystem.clone();
            tokio::spawn(async move {
                if let Err(e) = tokio::task::spawn_blocking(move || {
                    rt_updater::UpdateChecker::apply_and_exit(&path)
                })
                .await
                {
                    tracing::error!(error = %e, "update apply failed");
                    sub.lock().await.update_status = rt_updater::UpdateStatus::Failed {
                        error: e.to_string(),
                    };
                }
            });
            json_response(StatusCode::OK, r#"{"status":"applying"}"#.to_owned())
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

async fn config_page_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };

    let _lock = cs.write_lock.lock().await;
    let raw = match std::fs::read_to_string(&cs.path) {
        Ok(toml_str) => match toml::from_str::<crate::config::RawConfig>(&toml_str) {
            Ok(raw) => raw,
            Err(e) => {
                let body = format!("Error parsing config: {}", e);
                return text_response(StatusCode::INTERNAL_SERVER_ERROR, body);
            }
        },
        Err(e) => {
            let body = format!("Error reading config: {}", e);
            return text_response(StatusCode::INTERNAL_SERVER_ERROR, body);
        }
    };

    let restart_needed = state.subsystem.lock().await.restart_needed();
    let html = render_config_page(&raw, restart_needed);
    Html(html).into_response()
}

async fn config_json_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };

    let _lock = cs.write_lock.lock().await;
    match std::fs::read_to_string(&cs.path) {
        Ok(toml_str) => match toml::from_str::<crate::config::RawConfig>(&toml_str) {
            Ok(raw) => match serde_json::to_string(&raw) {
                Ok(json) => json_response(StatusCode::OK, json),
                Err(e) => json_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    serde_json::json!({"ok": false, "error": format!("JSON serialize error: {}", e)})
                        .to_string(),
                ),
            },
            Err(e) => json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"ok": false, "error": format!("TOML parse error: {}", e)})
                    .to_string(),
            ),
        },
        Err(e) => json_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            serde_json::json!({"ok": false, "error": format!("File read error: {}", e)})
                .to_string(),
        ),
    }
}

#[derive(serde::Deserialize)]
struct GeneralUpdate {
    display_name: Option<String>,
}

async fn post_config_general_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let update: GeneralUpdate = match parse_json_body(&body) {
        Ok(u) => u,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    let _lock = cs.write_lock.lock().await;
    match update_config_file(&cs, &state.subsystem, |raw| {
        raw.display_name = update.display_name;
        Ok(())
    })
    .await
    {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

#[derive(serde::Deserialize)]
struct ServerUpdate {
    base_url: Option<String>,
    forwarders_ws_path: Option<String>,
}

async fn post_config_server_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let update: ServerUpdate = match parse_json_body(&body) {
        Ok(u) => u,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    let base_url = match require_non_empty_trimmed("base_url", update.base_url) {
        Ok(base_url) => base_url,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };
    if let Err(err) = validate_base_url(&base_url) {
        return json_response(
            StatusCode::BAD_REQUEST,
            serde_json::json!({"ok": false, "error": err}).to_string(),
        );
    }
    let forwarders_ws_path = update.forwarders_ws_path.and_then(|path| {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        }
    });

    let _lock = cs.write_lock.lock().await;
    match update_config_file(&cs, &state.subsystem, |raw| {
        raw.server = Some(crate::config::RawServerConfig {
            base_url: Some(base_url),
            forwarders_ws_path,
        });
        Ok(())
    })
    .await
    {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

#[derive(serde::Deserialize)]
struct AuthUpdate {
    token_file: Option<String>,
}

async fn post_config_auth_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let update: AuthUpdate = match parse_json_body(&body) {
        Ok(u) => u,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    let token_file = match require_non_empty_trimmed("token_file", update.token_file) {
        Ok(token_file) => token_file,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };
    if let Err(err) = validate_token_file(&token_file) {
        return json_response(
            StatusCode::BAD_REQUEST,
            serde_json::json!({"ok": false, "error": err}).to_string(),
        );
    }

    let _lock = cs.write_lock.lock().await;
    match update_config_file(&cs, &state.subsystem, |raw| {
        raw.auth = Some(crate::config::RawAuthConfig {
            token_file: Some(token_file),
        });
        Ok(())
    })
    .await
    {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

#[derive(serde::Deserialize)]
struct JournalUpdate {
    sqlite_path: Option<String>,
    prune_watermark_pct: Option<u8>,
}

async fn post_config_journal_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let update: JournalUpdate = match parse_json_body(&body) {
        Ok(u) => u,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    let _lock = cs.write_lock.lock().await;
    match update_config_file(&cs, &state.subsystem, |raw| {
        raw.journal = Some(crate::config::RawJournalConfig {
            sqlite_path: update.sqlite_path,
            prune_watermark_pct: update.prune_watermark_pct,
        });
        Ok(())
    })
    .await
    {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

#[derive(serde::Deserialize)]
struct UplinkUpdate {
    batch_mode: Option<String>,
    batch_flush_ms: Option<u64>,
    batch_max_events: Option<u32>,
}

async fn post_config_uplink_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let update: UplinkUpdate = match parse_json_body(&body) {
        Ok(u) => u,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    let _lock = cs.write_lock.lock().await;
    match update_config_file(&cs, &state.subsystem, |raw| {
        raw.uplink = Some(crate::config::RawUplinkConfig {
            batch_mode: update.batch_mode,
            batch_flush_ms: update.batch_flush_ms,
            batch_max_events: update.batch_max_events,
        });
        Ok(())
    })
    .await
    {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

#[derive(serde::Deserialize)]
struct StatusHttpUpdate {
    bind: Option<String>,
}

async fn post_config_status_http_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let update: StatusHttpUpdate = match parse_json_body(&body) {
        Ok(u) => u,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    let _lock = cs.write_lock.lock().await;
    match update_config_file(&cs, &state.subsystem, |raw| {
        raw.status_http = Some(crate::config::RawStatusHttpConfig { bind: update.bind });
        Ok(())
    })
    .await
    {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

#[derive(serde::Deserialize)]
struct ReaderEntry {
    target: Option<String>,
    read_type: Option<String>,
    enabled: Option<bool>,
    local_fallback_port: Option<u16>,
}

#[derive(serde::Deserialize)]
struct ReadersUpdate {
    readers: Vec<ReaderEntry>,
}

async fn post_config_readers_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let update: ReadersUpdate = match parse_json_body(&body) {
        Ok(u) => u,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    if update.readers.is_empty() {
        return json_response(
            StatusCode::BAD_REQUEST,
            "{\"ok\":false,\"error\":\"at least one reader is required\"}".to_owned(),
        );
    }

    // Validate all targets before writing.
    for (i, r) in update.readers.iter().enumerate() {
        let target = match &r.target {
            Some(t) => t,
            None => {
                return json_response(
                    StatusCode::BAD_REQUEST,
                    serde_json::json!({"ok": false, "error": format!("readers[{}].target is required", i)}).to_string(),
                );
            }
        };
        if let Err(e) = crate::discovery::expand_target(target) {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": format!("readers[{}].target invalid: {}", i, e)}).to_string(),
            );
        }
    }

    let raw_readers: Vec<crate::config::RawReaderConfig> = update
        .readers
        .into_iter()
        .map(|r| crate::config::RawReaderConfig {
            target: r.target,
            read_type: r.read_type,
            enabled: r.enabled,
            local_fallback_port: r.local_fallback_port,
        })
        .collect();

    let _lock = cs.write_lock.lock().await;
    match update_config_file(&cs, &state.subsystem, |raw| {
        raw.readers = Some(raw_readers);
        Ok(())
    })
    .await
    {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn not_found_handler() -> Response {
    text_response(StatusCode::NOT_FOUND, "Not Found")
}

// ---------------------------------------------------------------------------
// Config page renderer
// ---------------------------------------------------------------------------

fn render_config_page(raw: &crate::config::RawConfig, restart_needed: bool) -> String {
    let display_name = html_escape(raw.display_name.as_deref().unwrap_or(""));

    let server = raw.server.as_ref();
    let base_url = html_escape(server.and_then(|s| s.base_url.as_deref()).unwrap_or(""));
    let ws_path = html_escape(
        server
            .and_then(|s| s.forwarders_ws_path.as_deref())
            .unwrap_or(""),
    );

    let auth = raw.auth.as_ref();
    let token_file = html_escape(auth.and_then(|a| a.token_file.as_deref()).unwrap_or(""));

    let journal = raw.journal.as_ref();
    let sqlite_path = html_escape(journal.and_then(|j| j.sqlite_path.as_deref()).unwrap_or(""));
    let prune_pct = journal
        .and_then(|j| j.prune_watermark_pct)
        .map(|v| v.to_string())
        .unwrap_or_default();

    let uplink = raw.uplink.as_ref();
    let batch_mode = html_escape(uplink.and_then(|u| u.batch_mode.as_deref()).unwrap_or(""));
    let batch_flush_ms = uplink
        .and_then(|u| u.batch_flush_ms)
        .map(|v| v.to_string())
        .unwrap_or_default();
    let batch_max_events = uplink
        .and_then(|u| u.batch_max_events)
        .map(|v| v.to_string())
        .unwrap_or_default();

    let status_http = raw.status_http.as_ref();
    let bind = html_escape(status_http.and_then(|s| s.bind.as_deref()).unwrap_or(""));

    // Build reader rows.
    let mut reader_rows = String::new();
    if let Some(readers) = &raw.readers {
        for r in readers {
            let target = html_escape(r.target.as_deref().unwrap_or(""));
            let read_type = r.read_type.as_deref().unwrap_or("raw");
            let enabled = r.enabled.unwrap_or(true);
            let checked = if enabled { "checked" } else { "" };
            let fallback_port = html_escape(
                &r.local_fallback_port
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
            );

            reader_rows.push_str(&format!(
                "<tr class=\"reader-row\">\
                 <td><input type=\"text\" name=\"target\" value=\"{target}\" required></td>\
                 <td><select name=\"read_type\"><option value=\"raw\"{raw_sel}>raw</option><option value=\"fsls\"{fsls_sel}>fsls</option></select></td>\
                 <td><input type=\"checkbox\" name=\"enabled\" {checked}></td>\
                 <td><input type=\"number\" name=\"local_fallback_port\" value=\"{fallback_port}\" min=\"1\" max=\"65535\"></td>\
                 <td><button type=\"button\" onclick=\"removeReader(this)\">Remove</button></td>\
                 </tr>",
                target = target,
                raw_sel = if read_type == "raw" { " selected" } else { "" },
                fsls_sel = if read_type == "fsls" { " selected" } else { "" },
                checked = checked,
                fallback_port = fallback_port,
            ));
        }
    }

    let restart_banner = if restart_needed {
        "<div class=\"banner warn\">Configuration changed. Restart the forwarder to apply changes.</div>"
    } else {
        ""
    };

    format!(
        r#"<!DOCTYPE html>
<html><head><title>Forwarder Configuration</title>
<style>
body{{font-family:system-ui,sans-serif;max-width:700px;margin:2rem auto;padding:0 1rem}}
h1{{margin-bottom:.5rem}}
h2{{margin-top:1.5rem;margin-bottom:.5rem;font-size:1.1rem}}
.card{{border:1px solid #ddd;border-radius:6px;padding:1rem;margin-bottom:1rem}}
label{{display:block;margin:.5rem 0 .2rem;font-weight:500}}
input[type="text"],input[type="number"],select{{width:100%;padding:.4rem;box-sizing:border-box;border:1px solid #ccc;border-radius:3px}}
button{{padding:.4rem .8rem;border:1px solid #ccc;border-radius:3px;cursor:pointer;background:#f8f8f8}}
button:hover{{background:#e8e8e8}}
button.save{{background:#d4edda;border-color:#155724;color:#155724}}
button.save:hover{{background:#c3e6cb}}
table{{border-collapse:collapse;width:100%}}
th,td{{text-align:left;padding:.3rem .4rem;border-bottom:1px solid #ddd}}
th{{font-weight:600;font-size:.9rem}}
.msg{{padding:.5rem;border-radius:4px;margin-top:.5rem;display:none}}
.msg.ok{{background:#d4edda;color:#155724;display:block}}
.msg.err{{background:#f8d7da;color:#721c24;display:block}}
.banner{{padding:.75rem 1rem;border-radius:4px;margin-bottom:1rem}}
.banner.warn{{background:#fff3cd;color:#856404;border:1px solid #ffc107}}
a{{color:#0366d6;text-decoration:none}}
a:hover{{text-decoration:underline}}
</style>
</head><body>
<h1>Forwarder Configuration</h1>
<p><a href="/">← Back to Status</a></p>
{restart_banner}

<div class="card">
<h2>General</h2>
<form id="form-general" onsubmit="return saveSection('/api/v1/config/general','form-general')">
<label>Display Name</label>
<input type="text" name="display_name" value="{display_name}">
<br><button type="submit" class="save">Save General</button>
<div id="msg-general" class="msg"></div>
</form>
</div>

<div class="card">
<h2>Server</h2>
<form id="form-server" onsubmit="return saveSection('/api/v1/config/server','form-server')">
<label>Base URL *</label>
<input type="text" name="base_url" value="{base_url}" required>
<label>Forwarders WS Path</label>
<input type="text" name="forwarders_ws_path" value="{ws_path}">
<br><button type="submit" class="save">Save Server</button>
<div id="msg-server" class="msg"></div>
</form>
</div>

<div class="card">
<h2>Auth</h2>
<form id="form-auth" onsubmit="return saveSection('/api/v1/config/auth','form-auth')">
<label>Token File Path *</label>
<input type="text" name="token_file" value="{token_file}" required>
<br><button type="submit" class="save">Save Auth</button>
<div id="msg-auth" class="msg"></div>
</form>
</div>

<div class="card">
<h2>Journal</h2>
<form id="form-journal" onsubmit="return saveSection('/api/v1/config/journal','form-journal')">
<label>SQLite Path</label>
<input type="text" name="sqlite_path" value="{sqlite_path}">
<label>Prune Watermark %</label>
<input type="number" name="prune_watermark_pct" value="{prune_pct}" min="0" max="100">
<br><button type="submit" class="save">Save Journal</button>
<div id="msg-journal" class="msg"></div>
</form>
</div>

<div class="card">
<h2>Uplink</h2>
<form id="form-uplink" onsubmit="return saveSection('/api/v1/config/uplink','form-uplink')">
<label>Batch Mode</label>
<input type="text" name="batch_mode" value="{batch_mode}">
<label>Batch Flush (ms)</label>
<input type="number" name="batch_flush_ms" value="{batch_flush_ms}" min="1">
<label>Batch Max Events</label>
<input type="number" name="batch_max_events" value="{batch_max_events}" min="1">
<br><button type="submit" class="save">Save Uplink</button>
<div id="msg-uplink" class="msg"></div>
</form>
</div>

<div class="card">
<h2>Status HTTP</h2>
<form id="form-status_http" onsubmit="return saveSection('/api/v1/config/status_http','form-status_http')">
<label>Bind Address</label>
<input type="text" name="bind" value="{bind}">
<br><button type="submit" class="save">Save Status HTTP</button>
<div id="msg-status_http" class="msg"></div>
</form>
</div>

<div class="card">
<h2>Readers</h2>
<table id="readers-table">
<tr><th>Target *</th><th>Read Type</th><th>Enabled</th><th>Fallback Port</th><th></th></tr>
{reader_rows}
</table>
<button type="button" onclick="addReader()">+ Add Reader</button>
<button type="button" class="save" onclick="saveReaders()">Save Readers</button>
<div id="msg-readers" class="msg"></div>
</div>

<script>
function saveSection(endpoint, formId) {{
  var form = document.getElementById(formId);
  var data = {{}};
  var inputs = form.querySelectorAll('input,select');
  for (var i = 0; i < inputs.length; i++) {{
    var inp = inputs[i];
    if (!inp.name) continue;
    if (inp.type === 'number') {{
      data[inp.name] = inp.value ? Number(inp.value) : null;
    }} else if (inp.type === 'checkbox') {{
      data[inp.name] = inp.checked;
    }} else {{
      data[inp.name] = inp.value || null;
    }}
  }}
  fetch(endpoint, {{
    method: 'POST',
    headers: {{'Content-Type': 'application/json'}},
    body: JSON.stringify(data)
  }}).then(function(r) {{ return r.json(); }}).then(function(j) {{
    var msgId = 'msg-' + formId.replace('form-','');
    var msg = document.getElementById(msgId);
    if (j.ok) {{
      msg.className = 'msg ok';
      msg.textContent = 'Saved. Restart to apply.';
      showRestartBanner();
    }} else {{
      msg.className = 'msg err';
      msg.textContent = j.error || 'Unknown error';
    }}
  }}).catch(function(e) {{
    alert('Request failed: ' + e);
  }});
  return false;
}}

function saveReaders() {{
  var rows = document.querySelectorAll('.reader-row');
  var readers = [];
  for (var i = 0; i < rows.length; i++) {{
    var row = rows[i];
    var entry = {{}};
    entry.target = row.querySelector('[name=target]').value || null;
    entry.read_type = row.querySelector('[name=read_type]').value || null;
    entry.enabled = row.querySelector('[name=enabled]').checked;
    var port = row.querySelector('[name=local_fallback_port]').value;
    entry.local_fallback_port = port ? Number(port) : null;
    readers.push(entry);
  }}
  fetch('/api/v1/config/readers', {{
    method: 'POST',
    headers: {{'Content-Type': 'application/json'}},
    body: JSON.stringify({{readers: readers}})
  }}).then(function(r) {{ return r.json(); }}).then(function(j) {{
    var msg = document.getElementById('msg-readers');
    if (j.ok) {{
      msg.className = 'msg ok';
      msg.textContent = 'Saved. Restart to apply.';
      showRestartBanner();
    }} else {{
      msg.className = 'msg err';
      msg.textContent = j.error || 'Unknown error';
    }}
  }}).catch(function(e) {{
    alert('Request failed: ' + e);
  }});
}}

function addReader() {{
  var table = document.getElementById('readers-table');
  var row = document.createElement('tr');
  row.className = 'reader-row';
  row.innerHTML = '<td><input type="text" name="target" required></td>' +
    '<td><select name="read_type"><option value="raw" selected>raw</option><option value="fsls">fsls</option></select></td>' +
    '<td><input type="checkbox" name="enabled" checked></td>' +
    '<td><input type="number" name="local_fallback_port" min="1" max="65535"></td>' +
    '<td><button type="button" onclick="removeReader(this)">Remove</button></td>';
  table.appendChild(row);
}}

function removeReader(el) {{
  el.closest('tr').remove();
}}

function showRestartBanner() {{
  if (!document.querySelector('.banner')) {{
    var banner = document.createElement('div');
    banner.className = 'banner warn';
    banner.textContent = 'Configuration changed. Restart the forwarder to apply changes.';
    document.querySelector('h1').after(banner);
  }}
}}
</script>
</body></html>"#,
        restart_banner = restart_banner,
        display_name = display_name,
        base_url = base_url,
        ws_path = ws_path,
        token_file = token_file,
        sqlite_path = sqlite_path,
        prune_pct = prune_pct,
        batch_mode = batch_mode,
        batch_flush_ms = batch_flush_ms,
        batch_max_events = batch_max_events,
        bind = bind,
        reader_rows = reader_rows,
    )
}
