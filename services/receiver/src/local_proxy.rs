//! Per-stream local TCP proxy.
//!
//! Opens a TCP listener on the assigned port for each subscribed stream.
//! Accepts local consumer connections and forwards events as they arrive via broadcast.
//! Emits CRLF-terminated IPICO lines for local TCP consumers.
//! Supports multiple simultaneous local consumers per stream.
//! Ports open as soon as subscriptions exist, even before server connection is established.

use rt_protocol::ReadEvent;
use std::net::SocketAddr;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

/// A handle to a running local proxy for one stream.
pub struct LocalProxy {
    pub port: u16,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl LocalProxy {
    /// Bind a TCP listener on `port` and start accepting local consumers.
    /// Pass the `broadcast::Sender<ReadEvent>` for the stream; each new TCP client
    /// gets its own `subscribe()` call from that sender.
    pub async fn bind(port: u16, event_tx: broadcast::Sender<ReadEvent>) -> std::io::Result<Self> {
        let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
        let listener = TcpListener::bind(addr).await?;
        info!(port, "local proxy bound");
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() { break; }
                    }
                    accept = listener.accept() => {
                        match accept {
                            Ok((stream, peer)) => {
                                debug!(?peer, port, "local consumer connected");
                                let rx = event_tx.subscribe();
                                tokio::spawn(serve_consumer(stream, rx));
                            }
                            Err(e) => { warn!(error=%e, "accept error"); }
                        }
                    }
                }
            }
        });

        Ok(Self { port, shutdown_tx })
    }

    /// Shut down the listener. Existing consumers will get EOF when the sender is dropped.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

/// Serve one local TCP consumer: forward each event's raw_read_line as bytes + CRLF.
async fn serve_consumer(mut stream: TcpStream, mut rx: broadcast::Receiver<ReadEvent>) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                let mut line = event.raw_read_line.into_bytes();
                line.extend_from_slice(b"\r\n");
                if stream.write_all(&line).await.is_err() {
                    break; // client disconnected
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(n, "local consumer lagged, events dropped");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rt_protocol::ReadEvent;
    use tokio::io::AsyncReadExt;

    fn make_event(raw: &str) -> ReadEvent {
        ReadEvent {
            forwarder_id: "f".to_owned(),
            reader_ip: "192.168.1.1".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "T".to_owned(),
            raw_read_line: raw.to_owned(),
            read_type: "RAW".to_owned(),
        }
    }

    /// Pick a free port.
    async fn free_port() -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let p = l.local_addr().unwrap().port();
        drop(l);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        p
    }

    #[tokio::test]
    async fn proxy_binds_and_accepts_connection() {
        let (tx, _rx) = broadcast::channel::<ReadEvent>(16);
        let port = free_port().await;
        let proxy = LocalProxy::bind(port, tx.clone()).await.unwrap();
        // Connect a client
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        // Wait for proxy to accept the connection
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Send an event
        tx.send(make_event("aa01,00:01:23.456")).unwrap();
        // Read from client
        let mut buf = vec![0u8; 64];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf))
            .await
            .expect("read should not timeout")
            .unwrap();
        let s = std::str::from_utf8(&buf[..n]).unwrap();
        assert!(s.contains("aa01,00:01:23.456"), "received: {s:?}");
        proxy.shutdown();
    }

    #[tokio::test]
    async fn proxy_forwards_exact_bytes() {
        let (tx, _rx) = broadcast::channel::<ReadEvent>(16);
        let port = free_port().await;
        let _proxy = LocalProxy::bind(port, tx.clone()).await.unwrap();
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let raw = "aa01,00:01:23.456";
        tx.send(make_event(raw)).unwrap();
        let mut buf = vec![0u8; 128];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf))
            .await
            .expect("read should not timeout")
            .unwrap();
        let received = std::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(received, format!("{raw}\r\n"));
    }

    #[tokio::test]
    async fn multiple_consumers_all_receive() {
        let (tx, _rx) = broadcast::channel::<ReadEvent>(16);
        let port = free_port().await;
        let _proxy = LocalProxy::bind(port, tx.clone()).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let mut c1 = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let mut c2 = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        let mut c3 = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        tx.send(make_event("line42")).unwrap();
        let mut buf = vec![0u8; 64];
        for (i, c) in [&mut c1, &mut c2, &mut c3].iter_mut().enumerate() {
            let n = tokio::time::timeout(std::time::Duration::from_secs(5), c.read(&mut buf))
                .await
                .unwrap_or_else(|_| panic!("consumer {i} read timed out"))
                .unwrap();
            let s = std::str::from_utf8(&buf[..n]).unwrap();
            assert!(s.contains("line42"), "consumer {i} did not receive: {s:?}");
        }
    }

    #[tokio::test]
    async fn proxy_open_before_events_arrive() {
        let (tx, _rx) = broadcast::channel::<ReadEvent>(16);
        let port = free_port().await;
        let proxy = LocalProxy::bind(port, tx).await.unwrap();
        // Client can connect immediately - port is open
        let _client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .expect("should connect even before any events");
        proxy.shutdown();
    }

    #[tokio::test]
    async fn shutdown_closes_listener() {
        let (tx, _rx) = broadcast::channel::<ReadEvent>(16);
        let port = free_port().await;
        let proxy = LocalProxy::bind(port, tx).await.unwrap();
        proxy.shutdown();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
        assert!(result.is_err(), "connection should fail after shutdown");
    }
}
