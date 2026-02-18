//! Local status HTTP server for Task 8.
//!
//! Provides:
//! - `GET /`              — read-only HTML status page
//! - `GET /healthz`       — always 200 OK (process is running)
//! - `GET /readyz`        — 200 when local subsystems ready, 503 otherwise
//! - `POST /api/v1/streams/{reader_ip}/reset-epoch`
//!   — bump stream epoch; 200 on success, 404 if unknown
//!
//! # Readiness contract
//! `/readyz` reflects local prerequisites only (config + SQLite + worker loops).
//! Uplink connectivity does NOT affect readiness.
//!
//! # Security
//! No authentication in v1. Status page is read-only.

use crate::storage::journal::Journal;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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
        let version = cfg.forwarder_version.clone();

        let server_subsystem = subsystem.clone();
        tokio::spawn(async move {
            run_server(listener, server_subsystem, journal, version).await;
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

fn from_hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn percent_decode_path_segment(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    return None;
                }
                let hi = from_hex_digit(bytes[i + 1])?;
                let lo = from_hex_digit(bytes[i + 2])?;
                out.push((hi << 4) | lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

// ---------------------------------------------------------------------------
// Server accept loop
// ---------------------------------------------------------------------------

async fn run_server<J: JournalAccess + Send + 'static>(
    listener: TcpListener,
    subsystem: Arc<Mutex<SubsystemStatus>>,
    journal: Arc<Mutex<J>>,
    version: String,
) {
    let version = Arc::new(version);
    while let Ok((stream, _)) = listener.accept().await {
        let subsystem = subsystem.clone();
        let journal = journal.clone();
        let version = version.clone();
        tokio::spawn(async move {
            handle_connection(stream, subsystem, journal, version).await;
        });
    }
}

// ---------------------------------------------------------------------------
// Request handler
// ---------------------------------------------------------------------------

async fn handle_connection<J: JournalAccess + Send + 'static>(
    mut stream: TcpStream,
    subsystem: Arc<Mutex<SubsystemStatus>>,
    journal: Arc<Mutex<J>>,
    version: Arc<String>,
) {
    // Read the request (limited to 4 KiB — sufficient for a simple HTTP/1.1 request line + headers)
    let mut buf = vec![0u8; 4096];
    let n = match stream.read(&mut buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };

    let request = match std::str::from_utf8(&buf[..n]) {
        Ok(s) => s,
        Err(_) => {
            send_response(&mut stream, 400, "text/plain", "Bad Request").await;
            return;
        }
    };

    // Parse the request line: METHOD PATH HTTP/1.1
    let first_line = match request.lines().next() {
        Some(l) => l,
        None => {
            send_response(&mut stream, 400, "text/plain", "Bad Request").await;
            return;
        }
    };

    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");

    match (method, path) {
        ("GET", "/healthz") => {
            send_response(&mut stream, 200, "text/plain", "ok").await;
        }
        ("GET", "/readyz") => {
            let ss = subsystem.lock().await;
            if ss.is_ready() {
                send_response(&mut stream, 200, "text/plain", "ready").await;
            } else {
                let reason = ss.reason.clone().unwrap_or_else(|| "not ready".to_owned());
                send_response(&mut stream, 503, "text/plain", &reason).await;
            }
        }
        ("GET", "/") => {
            let (ready, uplink_connected, forwarder_id, local_ip, readers) = {
                let ss = subsystem.lock().await;
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
                 <h1>Forwarder Status</h1>\
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
                version = *version,
                fwd_id = forwarder_id,
                local_ip = local_ip_display,
                rs = ready_state,
                rc = ready_class,
                us = uplink_state,
                uc = uplink_class,
                reader_rows = reader_rows,
            );
            send_response(&mut stream, 200, "text/html; charset=utf-8", &html).await;
        }
        ("POST", path)
            if path.starts_with("/api/v1/streams/") && path.ends_with("/reset-epoch") =>
        {
            // Extract reader_ip from: /api/v1/streams/{reader_ip}/reset-epoch
            let inner = &path["/api/v1/streams/".len()..path.len() - "/reset-epoch".len()];
            let reader_ip = match percent_decode_path_segment(inner) {
                Some(v) => v,
                None => {
                    send_response(
                        &mut stream,
                        400,
                        "text/plain",
                        "invalid percent-encoding in stream key",
                    )
                    .await;
                    return;
                }
            };

            let result = journal.lock().await.reset_epoch(&reader_ip);
            match result {
                Ok(new_epoch) => {
                    let body = format!("{{\"new_epoch\":{}}}", new_epoch);
                    send_response(&mut stream, 200, "application/json", &body).await;
                }
                Err(EpochResetError::NotFound) => {
                    send_response(&mut stream, 404, "text/plain", "stream not found").await;
                }
                Err(EpochResetError::Storage(e)) => {
                    send_response(&mut stream, 500, "text/plain", &e).await;
                }
            }
        }
        _ => {
            send_response(&mut stream, 404, "text/plain", "Not Found").await;
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP response helper
// ---------------------------------------------------------------------------

async fn send_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &str) {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Unknown",
    };

    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        status = status,
        status_text = status_text,
        content_type = content_type,
        len = body.len(),
        body = body,
    );

    let _ = stream.write_all(response.as_bytes()).await;
}
