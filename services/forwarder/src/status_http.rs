//! Local status HTTP server for Task 8.
//!
//! Provides:
//! - `GET /healthz`       — always 200 OK (process is running)
//! - `GET /readyz`        — 200 when local subsystems ready, 503 otherwise
//! - `GET /api/v1/status`  — current forwarder state as JSON
//! - `POST /api/v1/streams/{reader_ip}/reset-epoch`
//!   — bump stream epoch; 200 on success, 404 if unknown
//! - `GET /api/v1/config` — current config as JSON
//! - `POST /api/v1/config/{section}` — update a config section
//! - `POST /api/v1/restart` — trigger graceful restart; 404 if config editing not enabled;
//!   501 on non-Unix platforms
//! - `POST /api/v1/control/restart-service` — trigger graceful service restart
//! - `POST /api/v1/control/restart-device` — trigger host reboot (gated by config)
//! - `POST /api/v1/control/shutdown-device` — trigger host shutdown (gated by config)
//! - `GET /update/status`    — current rt-updater status as JSON
//! - `POST /update/apply`    — apply a staged update
//! - `POST /update/check`    — check for updates (respects update mode)
//! - All other routes fall back to the embedded SvelteKit UI
//!
//! # Readiness contract
//! `/readyz` reflects local prerequisites only (config + SQLite + worker loops).
//! Uplink connectivity does NOT affect readiness.
//!
//! # Security
//! No authentication in v1.

use crate::storage::journal::Journal;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use rt_updater::UpdateStatus;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::io::Write as _;
use std::net::{SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Instant;
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};

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
    /// The local port the forwarder listens on to re-expose reads from this reader.
    pub local_port: u16,
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
    pub update_mode: rt_updater::UpdateMode,
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
            update_mode: rt_updater::UpdateMode::default(),
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
            update_mode: rt_updater::UpdateMode::default(),
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
    ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
}

/// Holds the config file path and a write lock for read-modify-write operations.
pub struct ConfigState {
    pub path: std::path::PathBuf,
    pub(crate) write_lock: Mutex<()>,
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
    restart_signal: Option<Arc<Notify>>,
    ui_tx: tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
}

impl<J: JournalAccess + Send + 'static> Clone for AppState<J> {
    fn clone(&self) -> Self {
        Self {
            subsystem: self.subsystem.clone(),
            journal: self.journal.clone(),
            version: self.version.clone(),
            config_state: self.config_state.clone(),
            restart_signal: self.restart_signal.clone(),
            ui_tx: self.ui_tx.clone(),
        }
    }
}

impl StatusServer {
    /// Return the bound listen address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Return a clone of the internal subsystem status Arc.
    pub fn subsystem_arc(&self) -> Arc<Mutex<SubsystemStatus>> {
        self.subsystem.clone()
    }

    /// Return a clone of the UI event broadcast sender.
    pub fn ui_sender(&self) -> tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent> {
        self.ui_tx.clone()
    }

    /// Mark all local subsystems as ready.
    pub async fn set_ready(&self) {
        let mut ss = self.subsystem.lock().await;
        ss.ready = true;
        ss.reason = None;
        let _ = self
            .ui_tx
            .send(crate::ui_events::ForwarderUiEvent::StatusChanged {
                ready: ss.is_ready(),
                uplink_connected: ss.uplink_connected(),
                restart_needed: ss.restart_needed(),
            });
    }

    /// Mark that a restart is needed to apply saved config changes.
    pub async fn set_restart_needed(&self) {
        mark_restart_needed_and_emit(&self.subsystem, &self.ui_tx).await;
    }

    /// Return whether a restart is needed to apply saved config changes.
    pub async fn restart_needed(&self) -> bool {
        self.subsystem.lock().await.restart_needed()
    }

    /// Update the uplink connection state (does not affect readiness).
    pub async fn set_uplink_connected(&self, connected: bool) {
        let mut ss = self.subsystem.lock().await;
        ss.set_uplink_connected(connected);
        let _ = self
            .ui_tx
            .send(crate::ui_events::ForwarderUiEvent::StatusChanged {
                ready: ss.is_ready(),
                uplink_connected: connected,
                restart_needed: ss.restart_needed(),
            });
    }

    /// Set the forwarder ID (call once at startup).
    pub async fn set_forwarder_id(&self, id: &str) {
        self.subsystem.lock().await.forwarder_id = id.to_owned();
    }

    /// Set the detected local IP (call once at startup).
    pub async fn set_local_ip(&self, ip: Option<String>) {
        self.subsystem.lock().await.local_ip = ip;
    }

    /// Set the update mode (controls check-only vs check-and-download behavior).
    pub async fn set_update_mode(&self, mode: rt_updater::UpdateMode) {
        self.subsystem.lock().await.update_mode = mode;
    }

    /// Update the current rt-updater status (shown on `/update/status`).
    pub async fn set_update_status(&self, status: UpdateStatus) {
        self.subsystem.lock().await.update_status = status.clone();
        if let UpdateStatus::Downloaded { version } = status {
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::UpdateAvailable {
                    version,
                    current_version: env!("CARGO_PKG_VERSION").to_owned(),
                });
        }
    }

    /// Record the filesystem path of a downloaded update artifact ready to apply.
    pub async fn set_staged_update_path(&self, path: std::path::PathBuf) {
        self.subsystem.lock().await.staged_update_path = Some(path);
    }

    /// Pre-populate all configured reader IPs as Disconnected.
    ///
    /// Each entry is `(reader_addr, local_port)` where `reader_addr` is `"ip:port"`
    /// and `local_port` is the port the forwarder listens on to re-expose reads.
    pub async fn init_readers(&self, readers: &[(String, u16)]) {
        let mut ss = self.subsystem.lock().await;
        for (addr, local_port) in readers {
            ss.readers.entry(addr.clone()).or_insert(ReaderStatus {
                state: ReaderConnectionState::Disconnected,
                last_seen: None,
                reads_since_restart: 0,
                reads_total: 0,
                local_port: *local_port,
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
            let state_str = match &r.state {
                ReaderConnectionState::Connected => "connected",
                ReaderConnectionState::Connecting => "connecting",
                ReaderConnectionState::Disconnected => "disconnected",
            };
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                    ip: reader_ip.to_owned(),
                    state: state_str.to_owned(),
                    reads_session: r.reads_since_restart,
                    reads_total: r.reads_total,
                    last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                    local_port: r.local_port,
                });
        }
    }

    /// Record a successful chip read for a reader.
    pub async fn record_read(&self, reader_ip: &str) {
        let mut ss = self.subsystem.lock().await;
        if let Some(r) = ss.readers.get_mut(reader_ip) {
            r.reads_since_restart += 1;
            r.reads_total += 1;
            r.last_seen = Some(Instant::now());
            let _ = self
                .ui_tx
                .send(crate::ui_events::ForwarderUiEvent::ReaderUpdated {
                    ip: reader_ip.to_owned(),
                    state: match &r.state {
                        ReaderConnectionState::Connected => "connected",
                        ReaderConnectionState::Connecting => "connecting",
                        ReaderConnectionState::Disconnected => "disconnected",
                    }
                    .to_owned(),
                    reads_session: r.reads_since_restart,
                    reads_total: r.reads_total,
                    last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                    local_port: r.local_port,
                });
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

        let (ui_tx, _) = tokio::sync::broadcast::channel(256);
        let subsystem = Arc::new(Mutex::new(subsystem));
        let state = AppState {
            subsystem: subsystem.clone(),
            journal,
            version: Arc::new(cfg.forwarder_version),
            config_state: None,
            restart_signal: None,
            ui_tx: ui_tx.clone(),
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
            ui_tx,
        })
    }

    /// Start the status HTTP server with config editing support.
    pub async fn start_with_config<J: JournalAccess + Send + 'static>(
        cfg: StatusConfig,
        subsystem: SubsystemStatus,
        journal: Arc<Mutex<J>>,
        config_state: Arc<ConfigState>,
        restart_signal: Arc<Notify>,
    ) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind(&cfg.bind).await?;
        let local_addr = listener.local_addr()?;

        let (ui_tx, _) = tokio::sync::broadcast::channel(256);
        let subsystem = Arc::new(Mutex::new(subsystem));
        let state = AppState {
            subsystem: subsystem.clone(),
            journal,
            version: Arc::new(cfg.forwarder_version),
            config_state: Some(config_state),
            restart_signal: Some(restart_signal),
            ui_tx: ui_tx.clone(),
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
            ui_tx,
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
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
    mutate: impl FnOnce(&mut crate::config::RawConfig) -> Result<(), String>,
) -> Result<(), (u16, String)> {
    let _lock = config_state.write_lock.lock().await;

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

    mark_restart_needed_and_emit(subsystem, ui_tx).await;
    Ok(())
}

async fn mark_restart_needed_and_emit(
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
) {
    let mut ss = subsystem.lock().await;
    ss.set_restart_needed();
    let _ = ui_tx.send(crate::ui_events::ForwarderUiEvent::StatusChanged {
        ready: ss.is_ready(),
        uplink_connected: ss.uplink_connected(),
        restart_needed: true,
    });
}

/// Apply a config section update by name.
///
/// Dispatches to the right mutation logic based on `section`, validates the
/// payload, and calls `update_config_file` to persist the change.
///
/// Recognised sections: `"general"`, `"server"`, `"auth"`, `"journal"`,
/// `"uplink"`, `"status_http"`, `"control"`, `"update"`, `"readers"`.
pub async fn apply_section_update(
    section: &str,
    payload: &serde_json::Value,
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    ui_tx: &tokio::sync::broadcast::Sender<crate::ui_events::ForwarderUiEvent>,
) -> Result<(), (u16, String)> {
    require_object_payload(payload)?;

    match section {
        "general" => {
            let display_name = optional_string_field(payload, "display_name")?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.display_name = display_name;
                Ok(())
            })
            .await
        }
        "server" => {
            let base_url_opt = optional_string_field(payload, "base_url")?;
            let base_url =
                require_non_empty_trimmed("base_url", base_url_opt).map_err(bad_request_error)?;
            validate_base_url(&base_url).map_err(bad_request_error)?;
            let forwarders_ws_path = optional_string_field(payload, "forwarders_ws_path")?
                .and_then(|s| {
                    let trimmed = s.trim();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed.to_owned())
                    }
                });
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.server = Some(crate::config::RawServerConfig {
                    base_url: Some(base_url),
                    forwarders_ws_path,
                });
                Ok(())
            })
            .await
        }
        "auth" => {
            let token_file_opt = optional_string_field(payload, "token_file")?;
            let token_file = require_non_empty_trimmed("token_file", token_file_opt)
                .map_err(bad_request_error)?;
            validate_token_file(&token_file).map_err(bad_request_error)?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.auth = Some(crate::config::RawAuthConfig {
                    token_file: Some(token_file),
                });
                Ok(())
            })
            .await
        }
        "journal" => {
            let sqlite_path = optional_string_field(payload, "sqlite_path")?;
            let prune_watermark_pct = optional_u8_field(payload, "prune_watermark_pct")?;
            if let Some(pct) = prune_watermark_pct {
                if pct > 100 {
                    return Err(bad_request_error(
                        "prune_watermark_pct must be between 0 and 100",
                    ));
                }
            }
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.journal = Some(crate::config::RawJournalConfig {
                    sqlite_path,
                    prune_watermark_pct,
                });
                Ok(())
            })
            .await
        }
        "uplink" => {
            let batch_mode = optional_string_field(payload, "batch_mode")?;
            if let Some(ref mode) = batch_mode {
                if mode != "immediate" && mode != "batched" {
                    return Err(bad_request_error(
                        "batch_mode must be \"immediate\" or \"batched\"",
                    ));
                }
            }
            let batch_flush_ms = optional_u64_field(payload, "batch_flush_ms")?;
            let batch_max_events = optional_u32_field(payload, "batch_max_events")?;
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.uplink = Some(crate::config::RawUplinkConfig {
                    batch_mode,
                    batch_flush_ms,
                    batch_max_events,
                });
                Ok(())
            })
            .await
        }
        "status_http" => {
            let bind = optional_string_field(payload, "bind")?.and_then(|s| {
                let trimmed = s.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                }
            });
            if let Some(ref bind_addr) = bind {
                validate_status_bind(bind_addr).map_err(bad_request_error)?;
            }
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.status_http = Some(crate::config::RawStatusHttpConfig { bind });
                Ok(())
            })
            .await
        }
        "control" => {
            let allow_power_actions = optional_bool_field(payload, "allow_power_actions")?;
            let action = optional_string_field(payload, "action")?;
            if let Some(action) = action {
                return apply_control_action(&action, Some(config_state), None).await;
            }
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.control = Some(crate::config::RawControlConfig {
                    allow_power_actions,
                });
                Ok(())
            })
            .await
        }
        "update" => {
            let mode_str = optional_string_field(payload, "mode")?;
            let parsed_mode = match mode_str.as_ref() {
                Some(m) => serde_json::from_value::<rt_updater::UpdateMode>(
                    serde_json::Value::String(m.clone()),
                )
                .map_err(|_| {
                    (
                        400u16,
                        serde_json::json!({"ok": false, "error": format!(
                            "mode must be 'disabled', 'check-only', or 'check-and-download', got '{}'", m
                        )})
                        .to_string(),
                    )
                })?,
                None => rt_updater::UpdateMode::default(),
            };
            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.update = Some(crate::config::RawUpdateConfig { mode: mode_str });
                Ok(())
            })
            .await?;
            subsystem.lock().await.update_mode = parsed_mode;
            Ok(())
        }
        "readers" => {
            let readers_val = payload.get("readers").ok_or_else(|| {
                (
                    400u16,
                    serde_json::json!({"ok": false, "error": "readers field is required"})
                        .to_string(),
                )
            })?;
            let readers_arr = readers_val.as_array().ok_or_else(|| {
                (
                    400u16,
                    serde_json::json!({"ok": false, "error": "readers must be an array"})
                        .to_string(),
                )
            })?;

            if readers_arr.is_empty() {
                return Err((
                    400u16,
                    "{\"ok\":false,\"error\":\"at least one reader is required\"}".to_owned(),
                ));
            }

            let mut raw_readers = Vec::with_capacity(readers_arr.len());
            for (i, entry) in readers_arr.iter().enumerate() {
                let target = optional_string_field(entry, "target")?;

                let target_str = match &target {
                    Some(t) => t,
                    None => {
                        return Err((
                            400u16,
                            serde_json::json!({"ok": false, "error": format!("readers[{}].target is required", i)}).to_string(),
                        ));
                    }
                };

                if let Err(e) = crate::discovery::expand_target(target_str) {
                    return Err((
                        400u16,
                        serde_json::json!({"ok": false, "error": format!("readers[{}].target invalid: {}", i, e)}).to_string(),
                    ));
                }

                let enabled = optional_bool_field(entry, "enabled")?;
                let local_fallback_port = optional_u16_field(entry, "local_fallback_port")?;

                raw_readers.push(crate::config::RawReaderConfig {
                    target,
                    enabled,
                    local_fallback_port,
                });
            }

            update_config_file(config_state, subsystem, ui_tx, |raw| {
                raw.readers = Some(raw_readers);
                Ok(())
            })
            .await
        }
        _ => Err((
            400u16,
            serde_json::json!({"ok": false, "error": format!("unknown section: {}", section)})
                .to_string(),
        )),
    }
}

/// Read the config TOML file as JSON.
///
/// Returns `(config_json, restart_needed)` on success.
pub async fn read_config_json(
    config_state: &ConfigState,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
) -> Result<(serde_json::Value, bool), (u16, String)> {
    let _lock = config_state.write_lock.lock().await;

    let toml_str = std::fs::read_to_string(&config_state.path).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("File read error: {}", e)})
                .to_string(),
        )
    })?;

    let raw: crate::config::RawConfig = toml::from_str(&toml_str).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("TOML parse error: {}", e)})
                .to_string(),
        )
    })?;

    let json = serde_json::to_value(&raw).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("JSON serialize error: {}", e)})
                .to_string(),
        )
    })?;

    let restart_needed = subsystem.lock().await.restart_needed();
    Ok((json, restart_needed))
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

fn bad_request_error(message: impl Into<String>) -> (u16, String) {
    (
        400u16,
        serde_json::json!({"ok": false, "error": message.into()}).to_string(),
    )
}

fn require_object_payload(payload: &serde_json::Value) -> Result<(), (u16, String)> {
    if payload.is_object() {
        Ok(())
    } else {
        Err(bad_request_error("payload must be a JSON object"))
    }
}

fn optional_string_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<String>, (u16, String)> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(bad_request_error(format!(
            "{} must be a string or null",
            field
        ))),
    }
}

fn optional_bool_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<bool>, (u16, String)> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(bad_request_error(format!(
            "{} must be a boolean or null",
            field
        ))),
    }
}

fn optional_u64_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u64>, (u16, String)> {
    match payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => {
            let raw = value.as_u64().ok_or_else(|| {
                bad_request_error(format!("{} must be a non-negative integer or null", field))
            })?;
            Ok(Some(raw))
        }
    }
}

fn optional_u32_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u32>, (u16, String)> {
    let raw = optional_u64_field(payload, field)?;
    raw.map(|value| {
        u32::try_from(value)
            .map_err(|_| bad_request_error(format!("{} must be <= {}", field, u32::MAX)))
    })
    .transpose()
}

fn optional_u16_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u16>, (u16, String)> {
    let raw = optional_u64_field(payload, field)?;
    raw.map(|value| {
        u16::try_from(value)
            .map_err(|_| bad_request_error(format!("{} must be <= {}", field, u16::MAX)))
    })
    .transpose()
}

fn optional_u8_field(
    payload: &serde_json::Value,
    field: &str,
) -> Result<Option<u8>, (u16, String)> {
    let raw = optional_u64_field(payload, field)?;
    raw.map(|value| {
        u8::try_from(value)
            .map_err(|_| bad_request_error(format!("{} must be <= {}", field, u8::MAX)))
    })
    .transpose()
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

fn validate_status_bind(bind: &str) -> Result<(), String> {
    bind.parse::<SocketAddrV4>()
        .map(|_| ())
        .map_err(|_| "bind must be a valid IPv4 address with port (e.g. 0.0.0.0:8080)".to_owned())
}

fn config_not_available() -> Response {
    text_response(StatusCode::NOT_FOUND, "Config editing not available")
}

async fn restart_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    match &state.restart_signal {
        Some(signal) => {
            if cfg!(unix) {
                signal.notify_one();
                json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string())
            } else {
                json_response(
                    StatusCode::NOT_IMPLEMENTED,
                    serde_json::json!({
                        "ok": false,
                        "error": "restart not supported on non-unix platforms"
                    })
                    .to_string(),
                )
            }
        }
        None => config_not_available(),
    }
}

async fn read_allow_power_actions(config_state: &ConfigState) -> Result<bool, (u16, String)> {
    let _lock = config_state.write_lock.lock().await;
    let toml_str = std::fs::read_to_string(&config_state.path).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("File read error: {}", e)})
                .to_string(),
        )
    })?;
    let raw: crate::config::RawConfig = toml::from_str(&toml_str).map_err(|e| {
        (
            500u16,
            serde_json::json!({"ok": false, "error": format!("TOML parse error: {}", e)})
                .to_string(),
        )
    })?;
    Ok(raw
        .control
        .and_then(|c| c.allow_power_actions)
        .unwrap_or(false))
}

#[cfg(unix)]
fn map_power_action_command_result(
    systemctl_action: &'static str,
    result: std::io::Result<std::process::ExitStatus>,
) -> Result<(), (u16, String)> {
    match result {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => {
            tracing::error!(
                action = systemctl_action,
                exit_status = ?status.code(),
                "control action command exited with failure"
            );
            Err((
                500u16,
                serde_json::json!({
                    "ok": false,
                    "error": format!(
                        "control action command exited with failure: systemctl {}",
                        systemctl_action
                    )
                })
                .to_string(),
            ))
        }
        Err(e) => {
            tracing::error!(action = systemctl_action, error = %e, "control action command failed");
            Err((
                500u16,
                serde_json::json!({
                    "ok": false,
                    "error": format!("control action command failed: {}", e)
                })
                .to_string(),
            ))
        }
    }
}

#[cfg(unix)]
async fn run_device_power_action(systemctl_action: &'static str) -> Result<(), (u16, String)> {
    match tokio::task::spawn_blocking(move || {
        std::process::Command::new("systemctl")
            .arg(systemctl_action)
            .status()
    })
    .await
    {
        Ok(result) => map_power_action_command_result(systemctl_action, result),
        Err(e) => {
            tracing::error!(action = systemctl_action, error = %e, "control action task failed");
            Err((
                500u16,
                serde_json::json!({
                    "ok": false,
                    "error": format!("control action task failed: {}", e)
                })
                .to_string(),
            ))
        }
    }
}

#[cfg(not(unix))]
async fn run_device_power_action(_systemctl_action: &'static str) -> Result<(), (u16, String)> {
    Err((
        501u16,
        serde_json::json!({
            "ok": false,
            "error": "power actions not supported on non-unix platforms"
        })
        .to_string(),
    ))
}

pub async fn apply_control_action(
    action: &str,
    config_state: Option<&ConfigState>,
    restart_signal: Option<&Arc<Notify>>,
) -> Result<(), (u16, String)> {
    match action {
        "restart_service" => {
            let signal = restart_signal.ok_or_else(|| {
                (
                    404u16,
                    serde_json::json!({"ok": false, "error": "restart signal not available"})
                        .to_string(),
                )
            })?;
            if cfg!(unix) {
                signal.notify_one();
                Ok(())
            } else {
                Err((
                    501u16,
                    serde_json::json!({
                        "ok": false,
                        "error": "restart not supported on non-unix platforms"
                    })
                    .to_string(),
                ))
            }
        }
        "restart_device" | "shutdown_device" => {
            let cs = config_state.ok_or_else(|| {
                (
                    404u16,
                    serde_json::json!({"ok": false, "error": "Config editing not available"})
                        .to_string(),
                )
            })?;
            let allow_power_actions = read_allow_power_actions(cs).await?;
            if !allow_power_actions {
                return Err((
                    403u16,
                    serde_json::json!({"ok": false, "error": "power actions disabled"}).to_string(),
                ));
            }
            if !cfg!(unix) {
                return Err((
                    501u16,
                    serde_json::json!({
                        "ok": false,
                        "error": format!("{} not supported on non-unix platforms", action)
                    })
                    .to_string(),
                ));
            }
            let systemctl_action = if action == "restart_device" {
                "reboot"
            } else {
                "poweroff"
            };
            run_device_power_action(systemctl_action).await?;
            Ok(())
        }
        _ => Err(bad_request_error(format!(
            "unknown control action: {}",
            action
        ))),
    }
}

async fn control_restart_service_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    match apply_control_action("restart_service", None, state.restart_signal.as_ref()).await {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn control_restart_device_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    match apply_control_action("restart_device", Some(&cs), state.restart_signal.as_ref()).await {
        Ok(()) => json_response(
            StatusCode::OK,
            serde_json::json!({"ok": true, "status": "restart_device_scheduled"}).to_string(),
        ),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn control_shutdown_device_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    match apply_control_action("shutdown_device", Some(&cs), state.restart_signal.as_ref()).await {
        Ok(()) => json_response(
            StatusCode::OK,
            serde_json::json!({"ok": true, "status": "shutdown_device_scheduled"}).to_string(),
        ),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

fn get_config_state<J: JournalAccess + Send + 'static>(
    state: &AppState<J>,
) -> Option<Arc<ConfigState>> {
    state.config_state.clone()
}

#[derive(serde::Serialize)]
struct StatusJsonResponse {
    forwarder_id: String,
    version: String,
    ready: bool,
    ready_reason: Option<String>,
    uplink_connected: bool,
    restart_needed: bool,
    readers: Vec<ReaderStatusJson>,
}

#[derive(serde::Serialize)]
struct ReaderStatusJson {
    ip: String,
    state: String,
    reads_session: u64,
    reads_total: i64,
    last_seen_secs: Option<u64>,
    local_port: u16,
}

async fn status_json_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let ss = state.subsystem.lock().await;
    let mut readers: Vec<_> = ss
        .readers
        .iter()
        .map(|(ip, r)| {
            let state_str = match r.state {
                ReaderConnectionState::Connected => "connected",
                ReaderConnectionState::Connecting => "connecting",
                ReaderConnectionState::Disconnected => "disconnected",
            };
            ReaderStatusJson {
                ip: ip.clone(),
                state: state_str.to_owned(),
                reads_session: r.reads_since_restart,
                reads_total: r.reads_total,
                last_seen_secs: r.last_seen.map(|t| t.elapsed().as_secs()),
                local_port: r.local_port,
            }
        })
        .collect();
    readers.sort_by(|a, b| a.ip.cmp(&b.ip));

    let resp = StatusJsonResponse {
        forwarder_id: ss.forwarder_id.clone(),
        version: (*state.version).clone(),
        ready: ss.is_ready(),
        ready_reason: ss.reason.clone(),
        uplink_connected: ss.uplink_connected(),
        restart_needed: ss.restart_needed(),
        readers,
    };

    let body = serde_json::to_string(&resp)
        .unwrap_or_else(|_| r#"{"error":"serialization error"}"#.to_owned());
    json_response(StatusCode::OK, body)
}

async fn events_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> axum::response::sse::Sse<
    impl futures_util::stream::Stream<
        Item = Result<axum::response::sse::Event, std::convert::Infallible>,
    >,
> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use std::time::Duration;
    use tokio_stream::{wrappers::BroadcastStream, StreamExt};

    let rx = state.ui_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(event) => {
            let event_type = match &event {
                crate::ui_events::ForwarderUiEvent::StatusChanged { .. } => "status_changed",
                crate::ui_events::ForwarderUiEvent::ReaderUpdated { .. } => "reader_updated",
                crate::ui_events::ForwarderUiEvent::LogEntry { .. } => "log_entry",
                crate::ui_events::ForwarderUiEvent::UpdateAvailable { .. } => "update_available",
            };
            match serde_json::to_string(&event) {
                Ok(json) => Some(Ok(Event::default().event(event_type).data(json))),
                Err(_) => None,
            }
        }
        Err(_) => Some(Ok(Event::default().event("resync").data("{}"))),
    });

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keepalive"),
    )
}

fn build_router<J: JournalAccess + Send + 'static>(state: AppState<J>) -> Router {
    Router::new()
        .route("/healthz", get(healthz_handler))
        .route("/readyz", get(readyz_handler::<J>))
        .route(
            "/api/v1/streams/{reader_ip}/reset-epoch",
            post(reset_epoch_handler::<J>),
        )
        .route("/update/status", get(update_status_handler::<J>))
        .route("/update/apply", post(update_apply_handler::<J>))
        .route("/update/check", post(update_check_handler::<J>))
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
            "/api/v1/config/control",
            post(post_config_control_handler::<J>),
        )
        .route(
            "/api/v1/config/update",
            post(post_config_update_handler::<J>),
        )
        .route(
            "/api/v1/config/readers",
            post(post_config_readers_handler::<J>),
        )
        .route("/api/v1/restart", post(restart_handler::<J>))
        .route(
            "/api/v1/control/restart-service",
            post(control_restart_service_handler::<J>),
        )
        .route(
            "/api/v1/control/restart-device",
            post(control_restart_device_handler::<J>),
        )
        .route(
            "/api/v1/control/shutdown-device",
            post(control_shutdown_device_handler::<J>),
        )
        .route("/api/v1/status", get(status_json_handler::<J>))
        .route("/api/v1/events", get(events_handler::<J>))
        .fallback(crate::ui_server::serve_ui)
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
            if apply_via_restart_enabled() {
                schedule_process_restart();
                json_response(StatusCode::OK, r#"{"status":"restarting"}"#.to_owned())
            } else {
                let sub = state.subsystem.clone();
                tokio::spawn(async move {
                    match tokio::task::spawn_blocking(move || {
                        rt_updater::UpdateChecker::apply_and_exit(&path)
                    })
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            tracing::error!(error = %e, "update apply failed");
                            sub.lock().await.update_status = rt_updater::UpdateStatus::Failed {
                                error: e.to_string(),
                            };
                        }
                        Err(e) => {
                            tracing::error!(error = %e, "update apply task failed");
                            sub.lock().await.update_status = rt_updater::UpdateStatus::Failed {
                                error: e.to_string(),
                            };
                        }
                    }
                });
                json_response(StatusCode::OK, r#"{"status":"applying"}"#.to_owned())
            }
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

async fn update_check_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let update_mode = {
        let ss = state.subsystem.lock().await;
        ss.update_mode
    };

    let checker = match rt_updater::UpdateChecker::new(
        "iwismer",
        "rusty-timer",
        "forwarder",
        env!("CARGO_PKG_VERSION"),
    ) {
        Ok(c) => c,
        Err(e) => {
            let status = rt_updater::UpdateStatus::Failed {
                error: e.to_string(),
            };
            state.subsystem.lock().await.update_status = status.clone();
            let body = serde_json::to_string(&status).unwrap_or_else(|_| {
                r#"{"status":"failed","error":"serialization error"}"#.to_owned()
            });
            return json_response(StatusCode::INTERNAL_SERVER_ERROR, body);
        }
    };

    let check_result = checker.check().await;
    match check_result {
        Ok(rt_updater::UpdateStatus::Available { ref version }) => {
            state.subsystem.lock().await.update_status = rt_updater::UpdateStatus::Available {
                version: version.clone(),
            };

            if update_mode == rt_updater::UpdateMode::CheckAndDownload {
                match checker.download(version).await {
                    Ok(path) => {
                        let status = rt_updater::UpdateStatus::Downloaded {
                            version: version.clone(),
                        };
                        let mut ss = state.subsystem.lock().await;
                        ss.update_status = status.clone();
                        ss.staged_update_path = Some(path);
                        drop(ss);
                        let _ =
                            state
                                .ui_tx
                                .send(crate::ui_events::ForwarderUiEvent::UpdateAvailable {
                                    version: version.clone(),
                                    current_version: env!("CARGO_PKG_VERSION").to_owned(),
                                });
                        let body = serde_json::to_string(&status).unwrap_or_default();
                        json_response(StatusCode::OK, body)
                    }
                    Err(e) => {
                        let status = rt_updater::UpdateStatus::Failed {
                            error: e.to_string(),
                        };
                        state.subsystem.lock().await.update_status = status.clone();
                        let body = serde_json::to_string(&status).unwrap_or_default();
                        json_response(StatusCode::OK, body)
                    }
                }
            } else {
                let status = rt_updater::UpdateStatus::Available {
                    version: version.clone(),
                };
                let body = serde_json::to_string(&status).unwrap_or_default();
                json_response(StatusCode::OK, body)
            }
        }
        Ok(status) => {
            state.subsystem.lock().await.update_status = status.clone();
            let body = serde_json::to_string(&status).unwrap_or_default();
            json_response(StatusCode::OK, body)
        }
        Err(e) => {
            let status = rt_updater::UpdateStatus::Failed {
                error: e.to_string(),
            };
            state.subsystem.lock().await.update_status = status.clone();
            let body = serde_json::to_string(&status).unwrap_or_default();
            json_response(StatusCode::OK, body)
        }
    }
}

async fn config_json_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };

    match read_config_json(&cs, &state.subsystem).await {
        Ok((json_value, _restart_needed)) => {
            let json_str = serde_json::to_string(&json_value).unwrap_or_else(|e| {
                serde_json::json!({"ok": false, "error": format!("JSON serialize error: {}", e)})
                    .to_string()
            });
            json_response(StatusCode::OK, json_str)
        }
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_general_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let payload: serde_json::Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    match apply_section_update("general", &payload, &cs, &state.subsystem, &state.ui_tx).await {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_server_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let payload: serde_json::Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    match apply_section_update("server", &payload, &cs, &state.subsystem, &state.ui_tx).await {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_auth_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let payload: serde_json::Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    match apply_section_update("auth", &payload, &cs, &state.subsystem, &state.ui_tx).await {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_journal_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let payload: serde_json::Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    match apply_section_update("journal", &payload, &cs, &state.subsystem, &state.ui_tx).await {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_uplink_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let payload: serde_json::Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    match apply_section_update("uplink", &payload, &cs, &state.subsystem, &state.ui_tx).await {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_status_http_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let payload: serde_json::Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    match apply_section_update("status_http", &payload, &cs, &state.subsystem, &state.ui_tx).await {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_readers_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let payload: serde_json::Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    match apply_section_update("readers", &payload, &cs, &state.subsystem, &state.ui_tx).await {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_control_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let payload: serde_json::Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    match apply_section_update("control", &payload, &cs, &state.subsystem, &state.ui_tx).await {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

async fn post_config_update_handler<J: JournalAccess + Send + 'static>(
    State(state): State<AppState<J>>,
    body: Bytes,
) -> Response {
    let cs = match get_config_state(&state) {
        Some(cs) => cs,
        None => return config_not_available(),
    };
    let payload: serde_json::Value = match parse_json_body(&body) {
        Ok(v) => v,
        Err(err) => {
            return json_response(
                StatusCode::BAD_REQUEST,
                serde_json::json!({"ok": false, "error": err}).to_string(),
            )
        }
    };

    match apply_section_update("update", &payload, &cs, &state.subsystem, &state.ui_tx).await {
        Ok(()) => json_response(StatusCode::OK, serde_json::json!({"ok": true}).to_string()),
        Err((status_code, body)) => json_response(
            StatusCode::from_u16(status_code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            body,
        ),
    }
}

fn apply_via_restart_enabled() -> bool {
    apply_via_restart_from_env(std::env::var("RT_FORWARDER_UPDATE_APPLY_VIA_RESTART").ok())
}

fn apply_via_restart_from_env(value: Option<String>) -> bool {
    value.is_some_and(|raw| {
        let normalized = raw.trim().to_ascii_lowercase();
        matches!(normalized.as_str(), "1" | "true" | "yes" | "on")
    })
}

#[cfg(not(test))]
fn schedule_process_restart() {
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        std::process::exit(1);
    });
}

#[cfg(test)]
fn schedule_process_restart() {}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::{sleep, Duration};

    #[tokio::test]
    async fn update_apply_sets_failed_status_when_staged_file_missing() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "test".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server
            .set_update_status(UpdateStatus::Downloaded {
                version: "1.2.3".to_owned(),
            })
            .await;
        let temp = tempfile::tempdir().expect("tempdir");
        server
            .set_staged_update_path(temp.path().join("missing-forwarder-staged"))
            .await;

        let addr = server.local_addr();
        let base = format!("http://{}", addr);
        let client = reqwest::Client::new();

        let apply_resp = client
            .post(format!("{}/update/apply", base))
            .send()
            .await
            .expect("POST /update/apply");
        assert_eq!(apply_resp.status(), 200);

        let mut saw_failed = false;
        let mut last_body = String::new();
        for _ in 0..20 {
            let resp = client
                .get(format!("{}/update/status", base))
                .send()
                .await
                .expect("GET /update/status");
            last_body = resp.text().await.expect("response body");
            if last_body.contains(r#""status":"failed""#) {
                saw_failed = true;
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }

        assert!(
            saw_failed,
            "status never became failed, last response: {last_body}"
        );
    }

    #[tokio::test]
    async fn status_json_returns_forwarder_state() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        server.set_forwarder_id("fwd-abc123").await;
        server
            .init_readers(&[("192.168.1.10".to_owned(), 10010)])
            .await;
        server
            .update_reader_state("192.168.1.10", ReaderConnectionState::Connected)
            .await;

        let addr = server.local_addr();
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("http://{}/api/v1/status", addr))
            .send()
            .await
            .expect("GET /api/v1/status");
        assert_eq!(resp.status(), 200);

        let body: serde_json::Value = resp.json().await.expect("json body");
        assert_eq!(body["forwarder_id"], "fwd-abc123");
        assert_eq!(body["version"], "0.2.0");
        assert_eq!(body["ready"], true);
        assert_eq!(body["uplink_connected"], false);
        assert_eq!(body["restart_needed"], false);
        assert_eq!(body["readers"][0]["ip"], "192.168.1.10");
        assert_eq!(body["readers"][0]["state"], "connected");
    }

    #[tokio::test]
    async fn set_ready_broadcasts_status_changed() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::not_ready("starting".to_owned()),
        )
        .await
        .expect("start status server");

        let mut rx = server.ui_tx.subscribe();
        server.set_ready().await;

        let evt = tokio::time::timeout(Duration::from_millis(250), rx.recv())
            .await
            .expect("event timeout")
            .expect("recv event");
        match evt {
            crate::ui_events::ForwarderUiEvent::StatusChanged { ready, .. } => {
                assert!(ready);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_update_status_downloaded_broadcasts_update_available() {
        let server = StatusServer::start(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
        )
        .await
        .expect("start status server");

        let mut rx = server.ui_tx.subscribe();
        server
            .set_update_status(UpdateStatus::Downloaded {
                version: "1.2.3".to_owned(),
            })
            .await;

        let evt = tokio::time::timeout(Duration::from_millis(250), rx.recv())
            .await
            .expect("event timeout")
            .expect("recv event");
        match evt {
            crate::ui_events::ForwarderUiEvent::UpdateAvailable {
                version,
                current_version,
            } => {
                assert_eq!(version, "1.2.3");
                assert_eq!(current_version, env!("CARGO_PKG_VERSION"));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn config_save_broadcasts_status_changed_restart_needed() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        write!(
            config_file,
            r#"schema_version = 1
display_name = "Start Line"
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/fake-token"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        let mut rx = server.ui_tx.subscribe();
        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/config/general",
                server.local_addr()
            ))
            .header("content-type", "application/json")
            .body(r#"{"display_name":"Updated"}"#)
            .send()
            .await
            .expect("post config");
        assert_eq!(resp.status(), StatusCode::OK);

        let evt = tokio::time::timeout(Duration::from_millis(250), rx.recv())
            .await
            .expect("event timeout")
            .expect("recv event");
        match evt {
            crate::ui_events::ForwarderUiEvent::StatusChanged { restart_needed, .. } => {
                assert!(restart_needed);
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn config_update_endpoint_updates_runtime_mode_and_sets_restart_needed() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        write!(
            config_file,
            r#"schema_version = 1
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/fake-token"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        assert_eq!(
            server.subsystem.lock().await.update_mode,
            rt_updater::UpdateMode::CheckAndDownload
        );

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/config/update",
                server.local_addr()
            ))
            .header("content-type", "application/json")
            .body(r#"{"mode":"check-only"}"#)
            .send()
            .await
            .expect("post config update");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            server.subsystem.lock().await.update_mode,
            rt_updater::UpdateMode::CheckOnly
        );
        assert!(
            server.restart_needed().await,
            "restart_needed must be true after update config change"
        );
    }

    #[tokio::test]
    async fn config_update_endpoint_rejects_invalid_mode() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut config_file = NamedTempFile::new().expect("create temp config");
        write!(
            config_file,
            r#"schema_version = 1
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/fake-token"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");

        let restart_signal = Arc::new(Notify::new());
        let server = StatusServer::start_with_config(
            StatusConfig {
                bind: "127.0.0.1:0".to_owned(),
                forwarder_version: "0.2.0".to_owned(),
            },
            SubsystemStatus::ready(),
            Arc::new(Mutex::new(NoJournal)),
            Arc::new(ConfigState::new(config_file.path().to_path_buf())),
            restart_signal,
        )
        .await
        .expect("start status server");

        let client = reqwest::Client::new();
        let resp = client
            .post(format!(
                "http://{}/api/v1/config/update",
                server.local_addr()
            ))
            .header("content-type", "application/json")
            .body(r#"{"mode":"bogus"}"#)
            .send()
            .await
            .expect("post config update");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            server.subsystem.lock().await.update_mode,
            rt_updater::UpdateMode::CheckAndDownload
        );
    }

    #[cfg(unix)]
    #[test]
    fn power_action_command_result_returns_500_on_spawn_error() {
        let result = map_power_action_command_result(
            "reboot",
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "systemctl not found",
            )),
        );

        let (status, body) = result.expect_err("spawn errors must return an HTTP error");
        assert_eq!(status, 500);
        assert!(body.contains("control action command failed"));
    }

    #[cfg(unix)]
    #[test]
    fn power_action_command_result_returns_500_on_non_zero_exit() {
        use std::os::unix::process::ExitStatusExt;

        let status = std::process::ExitStatus::from_raw(1 << 8);
        let result = map_power_action_command_result("poweroff", Ok(status));

        let (http_status, body) = result.expect_err("non-zero exit must return an HTTP error");
        assert_eq!(http_status, 500);
        assert!(body.contains("control action command exited with failure"));
    }

    #[cfg(unix)]
    #[test]
    fn power_action_command_result_returns_ok_on_success_exit() {
        use std::os::unix::process::ExitStatusExt;

        let status = std::process::ExitStatus::from_raw(0);
        let result = map_power_action_command_result("reboot", Ok(status));
        assert!(result.is_ok());
    }

    #[test]
    fn apply_via_restart_env_parsing() {
        assert!(apply_via_restart_from_env(Some("1".to_owned())));
        assert!(apply_via_restart_from_env(Some("true".to_owned())));
        assert!(apply_via_restart_from_env(Some("YES".to_owned())));
        assert!(apply_via_restart_from_env(Some(" on ".to_owned())));
        assert!(!apply_via_restart_from_env(None));
        assert!(!apply_via_restart_from_env(Some("0".to_owned())));
        assert!(!apply_via_restart_from_env(Some("false".to_owned())));
    }
}
