//! Local status HTTP server for Task 8.
//!
//! Provides:
//! - `GET /`              — read-only HTML status page
//! - `GET /healthz`       — always 200 OK (process is running)
//! - `GET /readyz`        — 200 when local subsystems ready, 503 otherwise
//! - `POST /api/v1/streams/{reader_ip}/reset-epoch`
//!                        — bump stream epoch; 200 on success, 404 if unknown
//!
//! # Readiness contract
//! `/readyz` reflects local prerequisites only (config + SQLite + worker loops).
//! Uplink connectivity does NOT affect readiness.
//!
//! # Security
//! No authentication in v1. Status page is read-only.

use crate::storage::journal::Journal;
use std::net::SocketAddr;
use std::sync::Arc;
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
}

impl SubsystemStatus {
    /// Create a fully-ready subsystem status.
    pub fn ready() -> Self {
        SubsystemStatus {
            ready: true,
            reason: None,
            uplink_connected: false,
        }
    }

    /// Create a not-ready subsystem status with a reason.
    pub fn not_ready(reason: String) -> Self {
        SubsystemStatus {
            ready: false,
            reason: Some(reason),
            uplink_connected: false,
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
pub struct StatusServer {
    local_addr: SocketAddr,
}

impl StatusServer {
    /// Return the bound listen address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
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

        tokio::spawn(async move {
            run_server(listener, subsystem, journal, version).await;
        });

        Ok(StatusServer { local_addr })
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
        let (current_epoch, _) = self
            .current_epoch_and_next_seq(stream_key)
            .map_err(|e| {
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
}

/// Sentinel "no journal" implementation: every reset returns NotFound.
struct NoJournal;

impl JournalAccess for NoJournal {
    fn reset_epoch(&mut self, _stream_key: &str) -> Result<i64, EpochResetError> {
        Err(EpochResetError::NotFound)
    }
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
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let subsystem = subsystem.clone();
                let journal = journal.clone();
                let version = version.clone();
                tokio::spawn(async move {
                    handle_connection(stream, subsystem, journal, version).await;
                });
            }
            Err(_) => break,
        }
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
            let ss = subsystem.lock().await;
            let uplink_state = if ss.uplink_connected() {
                "connected"
            } else {
                "disconnected"
            };
            let ready_state = if ss.is_ready() { "ready" } else { "not-ready" };
            let html = format!(
                "<!DOCTYPE html><html><head><title>Forwarder Status</title></head>\
                 <body>\
                 <h1>Forwarder Status</h1>\
                 <p>Version: {version}</p>\
                 <p>Readiness: {ready}</p>\
                 <p>Uplink: {uplink}</p>\
                 </body></html>",
                version = *version,
                ready = ready_state,
                uplink = uplink_state,
            );
            send_response(&mut stream, 200, "text/html; charset=utf-8", &html).await;
        }
        ("POST", path) if path.starts_with("/api/v1/streams/") && path.ends_with("/reset-epoch") => {
            // Extract reader_ip from: /api/v1/streams/{reader_ip}/reset-epoch
            let inner = &path["/api/v1/streams/".len()..path.len() - "/reset-epoch".len()];
            let reader_ip = inner.to_owned();

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
