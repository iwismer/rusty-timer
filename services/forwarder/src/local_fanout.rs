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
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast};

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
    /// The bound local address, stored so that Drop can clean up the registry.
    local_addr: SocketAddr,
    /// Running total of messages dropped because consumers lagged behind the
    /// broadcast channel. Shared with the status store when wired via
    /// [`FanoutServer::set_drop_counter`].
    dropped: Arc<AtomicU64>,
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
        let local_addr = listener
            .local_addr()
            .expect("local_addr always succeeds after bind");
        registry().lock().await.insert(local_addr, tx.clone());

        Ok(FanoutServer {
            listener,
            tx,
            local_addr,
            dropped: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Replace the drop counter with a shared handle (e.g. the status store's
    /// `fanout_dropped_total`) so lag-induced drops are visible in status JSON.
    pub fn set_drop_counter(&mut self, counter: Arc<AtomicU64>) {
        self.dropped = counter;
    }

    /// Return the bound local address (useful when port 0 was used).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
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
        while let Ok((stream, _peer_addr)) = self.listener.accept().await {
            let rx = self.tx.subscribe();
            tokio::spawn(serve_consumer(stream, rx, Arc::clone(&self.dropped)));
        }
    }
}

impl Drop for FanoutServer {
    fn drop(&mut self) {
        // `registry()` uses tokio::sync::Mutex; Drop cannot be async so we use
        // try_lock().  If the lock is contended at drop time we accept the leak
        // rather than blocking or panicking — this is best-effort cleanup.
        if let Ok(mut reg) = registry().try_lock() {
            reg.remove(&self.local_addr);
        } else {
            tracing::warn!(addr = %self.local_addr, "FanoutServer::drop: registry lock contended, entry may leak");
        }
    }
}

// ---------------------------------------------------------------------------
// Per-consumer writer task
// ---------------------------------------------------------------------------

/// Drive one consumer connection: forward every broadcast message to the TCP
/// writer until the broadcast sender is dropped or the TCP write fails.
///
/// Messages missed because the consumer lagged behind the broadcast channel
/// are counted per consumer and accumulated into the shared `dropped` total.
async fn serve_consumer(
    mut stream: TcpStream,
    mut rx: broadcast::Receiver<Vec<u8>>,
    dropped: Arc<AtomicU64>,
) {
    let peer = stream.peer_addr().ok();
    let mut consumer_dropped: u64 = 0;
    loop {
        match rx.recv().await {
            Ok(data) => {
                if stream.write_all(&data).await.is_err() {
                    // Consumer disconnected — clean exit.
                    break;
                }
            }
            Err(broadcast::error::RecvError::Lagged(missed)) => {
                // Consumer is too slow; count the skipped messages, then
                // continue from the retained window.
                if consumer_dropped == 0 {
                    tracing::warn!(
                        peer = ?peer,
                        missed,
                        "fanout consumer lagged; dropping messages"
                    );
                }
                consumer_dropped += missed;
                dropped.fetch_add(missed, Ordering::Relaxed);
            }
            Err(broadcast::error::RecvError::Closed) => {
                // Channel closed (server shutting down).
                break;
            }
        }
    }
    if consumer_dropped > 0 {
        tracing::warn!(
            peer = ?peer,
            dropped = consumer_dropped,
            "fanout consumer disconnected after dropping messages"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio::io::AsyncReadExt;

    /// A consumer that lags behind the broadcast channel must increment the
    /// shared drop counter by the number of messages it missed.
    #[tokio::test]
    async fn slow_consumer_lag_increments_drop_counter() {
        // Mirror the production channel capacity (256) and overfill it before
        // the consumer task gets a chance to read: the first recv() then
        // deterministically observes Lagged(overflow).
        const CAPACITY: usize = 256;
        const SENT: u64 = 300;

        let (tx, rx) = broadcast::channel::<Vec<u8>>(CAPACITY);
        for i in 0..SENT {
            tx.send(vec![u8::try_from(i % 256).unwrap()]).unwrap();
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = TcpStream::connect(addr).await.unwrap();
        let (server_stream, _) = listener.accept().await.unwrap();

        let dropped = Arc::new(AtomicU64::new(0));
        let task = tokio::spawn(serve_consumer(server_stream, rx, Arc::clone(&dropped)));

        // Drain everything the consumer forwards, then close the channel so
        // the consumer task exits.
        drop(tx);
        let mut received = Vec::new();
        client.read_to_end(&mut received).await.unwrap();
        task.await.unwrap();

        let expected_dropped = SENT - CAPACITY as u64;
        assert_eq!(
            dropped.load(Ordering::Relaxed),
            expected_dropped,
            "drop counter must record the messages the lagged consumer missed"
        );
        assert_eq!(
            received.len() as u64,
            SENT - expected_dropped,
            "the retained window must still be delivered"
        );
    }

    /// Verify that dropping a FanoutServer removes its entry from the registry.
    #[tokio::test]
    async fn drop_removes_registry_entry() {
        let server = FanoutServer::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr();

        // Entry must be present right after bind.
        assert!(
            registry().lock().await.contains_key(&addr),
            "registry should contain entry after bind"
        );

        // Drop the server synchronously.
        drop(server);

        // Give the async runtime a moment in case of any scheduling nuances.
        tokio::task::yield_now().await;

        // Entry must be gone after drop.
        assert!(
            !registry().lock().await.contains_key(&addr),
            "registry should not contain entry after drop"
        );
    }
}
