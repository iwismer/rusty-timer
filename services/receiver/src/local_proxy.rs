//! Per-stream local TCP proxy.
//!
//! Opens a TCP listener on the assigned port for each subscribed stream.
//! Accepts local consumer connections and forwards events as they arrive via broadcast.
//! Emits raw frames exactly as received for local TCP consumers.
//! Supports multiple simultaneous local consumers per stream.
//! Ports open as soon as subscriptions exist, even before server connection is established.

use crate::db::{Db, ReceivedEvent};
use rt_domain::ReadEvent;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, broadcast};
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
        let listener = bind_listener(port).await?;
        let port = listener.local_addr()?.port();
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

    /// Bind a TCP listener backed by durable P2P `received_events`.
    ///
    /// New consumers first replay frames already stored for `stream_id`, ordered by
    /// `seq`. Live notifications carry a durable `seq`; the proxy reads the frame
    /// back from SQLite before writing it to the local TCP consumer.
    pub async fn bind_durable(
        port: u16,
        stream_id: String,
        db: Arc<Mutex<Db>>,
        durable_seq_tx: broadcast::Sender<i64>,
    ) -> std::io::Result<Self> {
        let listener = bind_listener(port).await?;
        let port = listener.local_addr()?.port();
        info!(port, %stream_id, "durable local proxy bound");
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
                                debug!(?peer, port, %stream_id, "durable local consumer connected");
                                let rx = durable_seq_tx.subscribe();
                                tokio::spawn(serve_durable_consumer(stream, stream_id.clone(), db.clone(), rx));
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

async fn bind_listener(port: u16) -> std::io::Result<TcpListener> {
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    TcpListener::bind(addr).await
}

/// Serve one local TCP consumer: forward each event's raw_frame bytes unchanged.
async fn serve_consumer(mut stream: TcpStream, mut rx: broadcast::Receiver<ReadEvent>) {
    loop {
        match rx.recv().await {
            Ok(event) => {
                if stream.write_all(&event.raw_frame).await.is_err() {
                    break; // client disconnected
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(
                    n,
                    "local consumer lagged, {n} events dropped — consumer will see a gap in data"
                );
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn serve_durable_consumer(
    mut stream: TcpStream,
    stream_id: String,
    db: Arc<Mutex<Db>>,
    mut rx: broadcast::Receiver<i64>,
) {
    let mut last_delivered_seq = 0;
    let replay = {
        let db = db.lock().await;
        db.load_received_events_after(&stream_id, last_delivered_seq)
    };
    match replay {
        Ok(events) => {
            if write_received_events(&mut stream, events, &mut last_delivered_seq)
                .await
                .is_err()
            {
                return;
            }
        }
        Err(e) => {
            warn!(error = %e, %stream_id, "failed to replay durable events");
            return;
        }
    }

    loop {
        match rx.recv().await {
            Ok(seq) if seq <= last_delivered_seq => {}
            Ok(seq) => {
                let events = {
                    let db = db.lock().await;
                    db.load_received_events_after(&stream_id, last_delivered_seq)
                };
                match events {
                    Ok(events) => {
                        if write_received_events(&mut stream, events, &mut last_delivered_seq)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, %stream_id, seq, "failed to drain durable events after live hint");
                        break;
                    }
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(
                    n,
                    "local consumer lagged, {n} durable event hints dropped; recovering from durable store"
                );
                let events = {
                    let db = db.lock().await;
                    db.load_received_events_after(&stream_id, last_delivered_seq)
                };
                match events {
                    Ok(events) => {
                        if write_received_events(&mut stream, events, &mut last_delivered_seq)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, %stream_id, "failed to recover lagged durable events");
                        break;
                    }
                }
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn write_received_events(
    stream: &mut TcpStream,
    events: Vec<ReceivedEvent>,
    last_delivered_seq: &mut i64,
) -> std::io::Result<()> {
    for event in events {
        let next_seq = *last_delivered_seq + 1;
        if event.seq < next_seq {
            continue;
        }
        if event.seq > next_seq {
            break;
        }
        stream.write_all(&event.raw_frame).await?;
        *last_delivered_seq = event.seq;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Db, ReceivedEventInsert};
    use rt_domain::ReadEvent;
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    fn make_event(raw: &[u8]) -> ReadEvent {
        ReadEvent {
            forwarder_id: "f".to_owned(),
            reader_ip: "192.168.1.1".to_owned(),
            stream_epoch: 1,
            seq: 1,
            reader_timestamp: "T".to_owned(),
            raw_frame: raw.to_vec(),
            read_type: "RAW".to_owned(),
        }
    }

    async fn insert_durable_event(
        db: &Arc<Mutex<Db>>,
        stream_id: &str,
        seq: i64,
        raw_frame: &[u8],
    ) {
        let inserted = db
            .lock()
            .await
            .insert_received_event(&ReceivedEventInsert {
                stream_id,
                seq,
                epoch: 1,
                raw_frame,
                read_kind: "raw",
                reader_timestamp: None,
                received_unix_ms: 1_700_000_000_000 + seq,
                dbf_delivered_unix_ms: None,
            })
            .unwrap();
        assert!(inserted);
    }

    #[tokio::test]
    async fn new_client_gets_replay() {
        let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
        // A real forwarder P2P stream_id (`ip:port`), not a parseable UUID.
        let stream_id = "127.0.0.1:10000";
        insert_durable_event(&db, stream_id, 2, b"second").await;
        insert_durable_event(&db, stream_id, 1, b"first").await;
        let (durable_tx, _rx) = broadcast::channel(16);
        let proxy = LocalProxy::bind_durable(0, stream_id.to_owned(), db, durable_tx)
            .await
            .unwrap();

        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
            .await
            .unwrap();
        let mut buf = vec![0u8; b"firstsecond".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut buf),
        )
        .await
        .expect("read should not timeout")
        .unwrap();

        assert_eq!(&buf, b"firstsecond");
        proxy.shutdown();
    }

    #[tokio::test]
    async fn live_reads_after_durable() {
        let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
        let stream_id = "22222222-2222-2222-2222-222222222222";
        insert_durable_event(&db, stream_id, 1, b"replay").await;
        let (durable_tx, _rx) = broadcast::channel(16);
        let proxy =
            LocalProxy::bind_durable(0, stream_id.to_owned(), db.clone(), durable_tx.clone())
                .await
                .unwrap();
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
            .await
            .unwrap();

        let mut replay_buf = vec![0u8; b"replay".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut replay_buf),
        )
        .await
        .expect("replay read should not timeout")
        .unwrap();
        assert_eq!(&replay_buf, b"replay");

        insert_durable_event(&db, stream_id, 2, b"live").await;
        durable_tx.send(2).unwrap();

        let mut live_buf = vec![0u8; b"live".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut live_buf),
        )
        .await
        .expect("live read should not timeout")
        .unwrap();

        assert_eq!(&live_buf, b"live");
        proxy.shutdown();
    }

    #[tokio::test]
    async fn out_of_order_durable_rows_wait_for_contiguous_gap() {
        let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
        let stream_id = "55555555-5555-5555-5555-555555555555";
        insert_durable_event(&db, stream_id, 1, b"one").await;
        let (durable_tx, _rx) = broadcast::channel(16);
        let proxy =
            LocalProxy::bind_durable(0, stream_id.to_owned(), db.clone(), durable_tx.clone())
                .await
                .unwrap();
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
            .await
            .unwrap();

        let mut replay_buf = vec![0u8; b"one".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut replay_buf),
        )
        .await
        .expect("replay read should not timeout")
        .unwrap();
        assert_eq!(&replay_buf, b"one");

        insert_durable_event(&db, stream_id, 3, b"three").await;
        durable_tx.send(3).unwrap();
        let mut early = [0u8; 1];
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                client.read_exact(&mut early),
            )
            .await
            .is_err(),
            "seq 3 must wait until seq 2 is durable"
        );

        insert_durable_event(&db, stream_id, 2, b"two").await;
        durable_tx.send(2).unwrap();

        let mut contiguous_buf = vec![0u8; b"twothree".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut contiguous_buf),
        )
        .await
        .expect("contiguous read should not timeout")
        .unwrap();
        assert_eq!(&contiguous_buf, b"twothree");
        proxy.shutdown();
    }

    #[tokio::test]
    async fn snapshot_live_overlap_does_not_duplicate() {
        let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
        let stream_id = "33333333-3333-3333-3333-333333333333";
        let (durable_tx, rx) = broadcast::channel(16);
        insert_durable_event(&db, stream_id, 1, b"once").await;
        durable_tx.send(1).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (server_stream, _) = listener.accept().await.unwrap();
        let handle = tokio::spawn(serve_durable_consumer(
            server_stream,
            stream_id.to_owned(),
            db,
            rx,
        ));

        let mut buf = vec![0u8; b"once".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut buf),
        )
        .await
        .expect("read should not timeout")
        .unwrap();
        assert_eq!(&buf, b"once");

        let mut extra = [0u8; 1];
        assert!(
            tokio::time::timeout(
                std::time::Duration::from_millis(100),
                client.read_exact(&mut extra),
            )
            .await
            .is_err(),
            "overlapping live notification should not replay the same frame twice"
        );
        drop(durable_tx);
        handle.abort();
    }

    #[tokio::test]
    async fn lagged_live_hint_recovers_from_durable_store() {
        let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
        let stream_id = "44444444-4444-4444-4444-444444444444";
        insert_durable_event(&db, stream_id, 1, b"one").await;
        let (durable_tx, _rx) = broadcast::channel(1);
        let proxy =
            LocalProxy::bind_durable(0, stream_id.to_owned(), db.clone(), durable_tx.clone())
                .await
                .unwrap();
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
            .await
            .unwrap();

        let mut replay_buf = vec![0u8; b"one".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut replay_buf),
        )
        .await
        .expect("replay read should not timeout")
        .unwrap();
        assert_eq!(&replay_buf, b"one");

        insert_durable_event(&db, stream_id, 2, b"two").await;
        insert_durable_event(&db, stream_id, 3, b"three").await;
        insert_durable_event(&db, stream_id, 4, b"four").await;
        durable_tx.send(2).unwrap();
        durable_tx.send(3).unwrap();
        durable_tx.send(4).unwrap();

        let mut recovered_buf = vec![0u8; b"twothreefour".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut recovered_buf),
        )
        .await
        .expect("lag recovery read should not timeout")
        .unwrap();
        assert_eq!(&recovered_buf, b"twothreefour");
        proxy.shutdown();
    }

    #[tokio::test]
    async fn proxy_binds_and_accepts_connection() {
        let (tx, _rx) = broadcast::channel::<ReadEvent>(16);
        let proxy = LocalProxy::bind(0, tx.clone()).await.unwrap();
        let port = proxy.port;
        // Connect a client
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        // Wait for proxy to accept the connection
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Send an event
        tx.send(make_event(b"aa01,00:01:23.456")).unwrap();
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
        let _proxy = LocalProxy::bind(0, tx.clone()).await.unwrap();
        let port = _proxy.port;
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let raw = b"aa01,00:01:23.456";
        tx.send(make_event(raw)).unwrap();
        let mut buf = vec![0u8; 128];
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), client.read(&mut buf))
            .await
            .expect("read should not timeout")
            .unwrap();
        assert_eq!(&buf[..n], raw);
    }

    #[tokio::test]
    async fn multiple_consumers_all_receive() {
        let (tx, _rx) = broadcast::channel::<ReadEvent>(16);
        let _proxy = LocalProxy::bind(0, tx.clone()).await.unwrap();
        let port = _proxy.port;
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
        tx.send(make_event(b"line42")).unwrap();
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
        let proxy = LocalProxy::bind(0, tx).await.unwrap();
        let port = proxy.port;
        // Client can connect immediately - port is open
        let _client = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .expect("should connect even before any events");
        proxy.shutdown();
    }

    #[tokio::test]
    async fn shutdown_closes_listener() {
        let (tx, _rx) = broadcast::channel::<ReadEvent>(16);
        let proxy = LocalProxy::bind(0, tx).await.unwrap();
        let port = proxy.port;
        proxy.shutdown();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let result = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}")).await;
        assert!(result.is_err(), "connection should fail after shutdown");
    }
}
