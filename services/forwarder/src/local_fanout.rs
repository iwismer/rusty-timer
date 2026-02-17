//! Local raw TCP fanout for Task 8.
//!
//! Each `FanoutServer` listens on a TCP port and forwards every pushed byte
//! payload to all currently-connected consumers.  The fanout preserves exact
//! bytes — no line-ending rewrite, no framing, no normalization.
//!
//! Multiple simultaneous consumers are supported.  When a consumer disconnects,
//! it is silently removed; remaining consumers are unaffected.
//!
//! # Usage
//! ```rust,no_run
//! # async fn example() {
//! use forwarder::local_fanout::FanoutServer;
//! let server = FanoutServer::bind("127.0.0.1:10005").await.unwrap();
//! let addr = server.local_addr();
//! tokio::spawn(async move { server.run().await });
//! FanoutServer::push_to_addr(addr, b"raw bytes".to_vec()).await.unwrap();
//! # }
//! ```

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Mutex};

// ---------------------------------------------------------------------------
// Public error type
// ---------------------------------------------------------------------------

/// Errors that can arise from fanout operations.
#[derive(Debug)]
pub enum FanoutError {
    /// Failed to bind to the requested address (e.g. port already in use).
    BindFailed(std::io::Error),
    /// Internal channel send error.
    Send(String),
    /// Server not found at the given address.
    NotFound,
}

impl std::fmt::Display for FanoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FanoutError::BindFailed(e) => write!(f, "bind failed: {}", e),
            FanoutError::Send(s) => write!(f, "send error: {}", s),
            FanoutError::NotFound => write!(f, "fanout server not found"),
        }
    }
}

impl std::error::Error for FanoutError {}

// ---------------------------------------------------------------------------
// Global registry: SocketAddr → broadcast sender
// ---------------------------------------------------------------------------

type BroadcastSender = broadcast::Sender<Vec<u8>>;

/// Global map from listen address → broadcast sender, so that
/// `FanoutServer::push_to_addr` can reach a running server.
static REGISTRY: std::sync::OnceLock<Arc<Mutex<HashMap<SocketAddr, BroadcastSender>>>> =
    std::sync::OnceLock::new();

fn registry() -> &'static Arc<Mutex<HashMap<SocketAddr, BroadcastSender>>> {
    REGISTRY.get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
}

// ---------------------------------------------------------------------------
// FanoutServer
// ---------------------------------------------------------------------------

/// A local TCP fanout server that broadcasts raw bytes to all connected consumers.
pub struct FanoutServer {
    listener: TcpListener,
    /// Broadcast channel: every push goes to all active consumers.
    tx: BroadcastSender,
}

impl FanoutServer {
    /// Bind a new fanout listener on `addr` (use `"127.0.0.1:0"` to let the
    /// OS choose a free port).
    ///
    /// Returns `FanoutError::BindFailed` if the address is already in use.
    pub async fn bind(addr: &str) -> Result<Self, FanoutError> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(FanoutError::BindFailed)?;
        // Capacity: 256 pending payloads per consumer before overrun.
        let (tx, _rx) = broadcast::channel(256);

        // Register in the global map so push_to_addr can reach us.
        let local_addr = listener.local_addr().expect("local_addr always succeeds after bind");
        registry().lock().await.insert(local_addr, tx.clone());

        Ok(FanoutServer { listener, tx })
    }

    /// Return the bound local address (useful when port 0 was used).
    pub fn local_addr(&self) -> SocketAddr {
        self.listener
            .local_addr()
            .expect("local_addr always succeeds after bind")
    }

    /// Broadcast `data` to all consumers currently subscribed to `addr`.
    ///
    /// Returns `FanoutError::NotFound` if no server is registered at `addr`.
    /// Returns `Ok(())` even if there are zero subscribers.
    pub async fn push_to_addr(addr: SocketAddr, data: Vec<u8>) -> Result<(), FanoutError> {
        let reg = registry().lock().await;
        match reg.get(&addr) {
            Some(tx) => {
                // If there are no receivers the broadcast channel returns Err,
                // but we treat zero-subscriber case as success.
                let _ = tx.send(data);
                Ok(())
            }
            None => Err(FanoutError::NotFound),
        }
    }

    /// Run the fanout accept loop.  This consumes `self` and runs until the
    /// listener is dropped.
    pub async fn run(self) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _peer_addr)) => {
                    let rx = self.tx.subscribe();
                    tokio::spawn(serve_consumer(stream, rx));
                }
                Err(_) => break,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Per-consumer writer task
// ---------------------------------------------------------------------------

/// Drive one consumer connection: forward every broadcast message to the TCP
/// writer until the broadcast sender is dropped or the TCP write fails.
async fn serve_consumer(mut stream: TcpStream, mut rx: broadcast::Receiver<Vec<u8>>) {
    loop {
        match rx.recv().await {
            Ok(data) => {
                if stream.write_all(&data).await.is_err() {
                    // Consumer disconnected — clean exit.
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                // Consumer is too slow; skip missed messages and continue.
                continue;
            }
            Err(broadcast::error::RecvError::Closed) => {
                // Channel closed (server shutting down).
                break;
            }
        }
    }
}
