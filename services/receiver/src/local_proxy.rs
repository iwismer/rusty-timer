//! Per-stream local TCP proxy.
//!
//! Opens a TCP listener on the assigned port for each subscribed stream.
//! Accepts local consumer connections and forwards events as they arrive via broadcast.
//! Emits raw frames exactly as received for local TCP consumers.
//! Supports multiple simultaneous local consumers per stream.
//! Ports open as soon as subscriptions exist, even before server connection is established.

use crate::db::ReceivedEvent;
use crate::p2p_session::DurableBatch;
use crate::read_pool::ReadSource;
use crate::retention::ProxyConsumerCursors;
use rt_domain::ReadEvent;
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
        read: ReadSource,
        durable_seq_tx: broadcast::Sender<DurableBatch>,
        consumer_cursors: ProxyConsumerCursors,
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
                                tokio::spawn(serve_durable_consumer(
                                    stream,
                                    stream_id.clone(),
                                    read.clone(),
                                    rx,
                                    consumer_cursors.clone(),
                                ));
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

/// Rows fetched per drain chunk. A pooled read connection is released after
/// each chunk — never held across the TCP `write_all` — so a slow consumer
/// cannot stall WAL checkpointing.
const DRAIN_CHUNK_ROWS: usize = 4096;

/// Drain durable rows after `last_delivered_seq` to the consumer in bounded
/// chunks. Returns `Err(())` when the consumer disconnected or a DB error
/// makes continuing pointless.
async fn drain_durable_chunks(
    stream: &mut TcpStream,
    stream_id: &str,
    read: &ReadSource,
    last_delivered_seq: &mut i64,
) -> Result<(), ()> {
    loop {
        let after_seq = *last_delivered_seq;
        let stream_id_owned = stream_id.to_owned();
        let events = read
            .run(move |db| {
                db.load_received_events_after_limited(&stream_id_owned, after_seq, DRAIN_CHUNK_ROWS)
            })
            .await;
        let events = match events {
            Ok(events) => events,
            Err(e) => {
                warn!(error = %e, %stream_id, "failed to load durable events for consumer");
                return Err(());
            }
        };
        if events.is_empty() {
            return Ok(());
        }
        let fetched = events.len();
        let last_seq_in_chunk = events.last().map_or(after_seq, |event| event.seq);
        // Connection released above; only now write to the (possibly slow)
        // consumer.
        if write_received_events(stream, events, last_delivered_seq)
            .await
            .is_err()
        {
            return Err(());
        }
        // The contiguous prefix ended inside the chunk: either a transient
        // arrival gap (the next hint resumes) or a *permanent* one — rows
        // pruned by retention after this consumer read its start watermark,
        // or a P2P gap notice that jumped the durable cursor. Permanent gaps
        // must jump the consumer cursor (the analog of the P2P gap jump),
        // otherwise the consumer stalls forever and its registered cursor
        // wedges retention for the stream.
        if *last_delivered_seq < last_seq_in_chunk {
            match durable_gap_jump_target(stream_id, read, *last_delivered_seq).await {
                Ok(Some(jump_to)) => {
                    debug!(
                        %stream_id,
                        from = *last_delivered_seq,
                        jump_to,
                        "durable consumer jumping permanent gap (prune/gap-notice)"
                    );
                    *last_delivered_seq = jump_to;
                    continue;
                }
                Ok(None) => return Ok(()), // transient gap; wait for a hint
                Err(()) => return Err(()),
            }
        }
        if fetched < DRAIN_CHUNK_ROWS {
            return Ok(());
        }
    }
}

/// When a consumer is stalled at `cursor`, resolve the highest *permanent*
/// gap jump covering it: the retention watermark (rows at or below it are
/// deleted) and any recorded P2P gap marker whose unavailable range covers
/// `cursor + 1`. Returns `None` when the gap is not provably permanent.
async fn durable_gap_jump_target(
    stream_id: &str,
    read: &ReadSource,
    cursor: i64,
) -> Result<Option<i64>, ()> {
    let stream_id_owned = stream_id.to_owned();
    let target = read
        .run(move |db| {
            let mut target = db.load_pruned_through_seq(&stream_id_owned)?;
            for marker in db.load_gap_markers(&stream_id_owned)? {
                // The marker says seqs in (requested_after_seq,
                // earliest_available_seq) are permanently unavailable; if the
                // consumer's next seq falls in that range, jump to the marker
                // boundary exactly like the P2P cursor did.
                let jump_to = marker.earliest_available_seq.saturating_sub(1);
                if marker.requested_after_seq <= cursor && jump_to > cursor {
                    target = target.max(jump_to);
                }
            }
            Ok(target)
        })
        .await;
    match target {
        Ok(target) if target > cursor => Ok(Some(target)),
        Ok(_) => Ok(None),
        Err(e) => {
            warn!(error = %e, %stream_id, "failed to resolve durable gap jump for consumer");
            Err(())
        }
    }
}

/// RAII registration of one consumer's replay cursor in the shared registry
/// (retention floor input). Deregisters on drop so a disconnected consumer
/// stops holding the prune floor down.
struct ConsumerCursorGuard {
    registry: ProxyConsumerCursors,
    stream_id: String,
    id: u64,
}

impl ConsumerCursorGuard {
    fn register(registry: ProxyConsumerCursors, stream_id: String, cursor: i64) -> Self {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let mut map = registry.lock().expect("proxy cursor registry poisoned");
            let _ = map.entry(stream_id.clone()).or_default().insert(id, cursor);
        }
        Self {
            registry,
            stream_id,
            id,
        }
    }

    fn update(&self, cursor: i64) {
        let mut map = self
            .registry
            .lock()
            .expect("proxy cursor registry poisoned");
        if let Some(consumers) = map.get_mut(&self.stream_id) {
            let _ = consumers.insert(self.id, cursor);
        }
    }
}

impl Drop for ConsumerCursorGuard {
    fn drop(&mut self) {
        let mut map = self
            .registry
            .lock()
            .expect("proxy cursor registry poisoned");
        if let Some(consumers) = map.get_mut(&self.stream_id) {
            let _ = consumers.remove(&self.id);
            if consumers.is_empty() {
                let _ = map.remove(&self.stream_id);
            }
        }
    }
}

async fn serve_durable_consumer(
    mut stream: TcpStream,
    stream_id: String,
    read: ReadSource,
    mut rx: broadcast::Receiver<DurableBatch>,
    consumer_cursors: ProxyConsumerCursors,
) {
    // Start replay at the retention watermark, not 0: seqs at or below it
    // were pruned, and contiguous delivery would otherwise wait forever for
    // a deleted seq (analogous to gap markers jumping the P2P cursor).
    let mut last_delivered_seq = {
        let sid = stream_id.clone();
        match read.run(move |db| db.load_pruned_through_seq(&sid)).await {
            Ok(seq) => seq,
            Err(e) => {
                warn!(error = %e, %stream_id, "failed to load retention watermark for consumer");
                return;
            }
        }
    };
    let guard =
        ConsumerCursorGuard::register(consumer_cursors, stream_id.clone(), last_delivered_seq);
    if drain_durable_chunks(&mut stream, &stream_id, &read, &mut last_delivered_seq)
        .await
        .is_err()
    {
        return;
    }
    guard.update(last_delivered_seq);

    loop {
        match rx.recv().await {
            Ok(batch) if batch.through_seq <= last_delivered_seq => {}
            Ok(_batch) => {
                if drain_durable_chunks(&mut stream, &stream_id, &read, &mut last_delivered_seq)
                    .await
                    .is_err()
                {
                    break;
                }
                guard.update(last_delivered_seq);
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(
                    n,
                    "local consumer lagged, {n} durable event hints dropped; recovering from durable store"
                );
                if drain_durable_chunks(&mut stream, &stream_id, &read, &mut last_delivered_seq)
                    .await
                    .is_err()
                {
                    break;
                }
                guard.update(last_delivered_seq);
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

    fn hint(through_seq: i64) -> DurableBatch {
        DurableBatch {
            through_seq,
            inserted: Arc::new(Vec::new()),
        }
    }

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
                chip_id: None,
            })
            .unwrap();
        assert!(inserted);
    }

    #[tokio::test]
    async fn new_consumer_after_pruning_receives_retained_rows() {
        // Regression test for the retention hang: seqs 1..=2 were pruned; a
        // new consumer must initialize its cursor from pruned_through_seq and
        // receive the retained contiguous rows instead of waiting forever for
        // the deleted seq 1.
        let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
        let stream_id = "127.0.0.1:10600";
        insert_durable_event(&db, stream_id, 3, b"three").await;
        insert_durable_event(&db, stream_id, 4, b"four").await;
        {
            let guard = db.lock().await;
            // Simulate a completed prune of seqs 1..=2.
            guard
                .raw_execute_for_test(
                    "INSERT INTO retention (stream_id, pruned_through_seq) VALUES (?1, 2)",
                    rusqlite::params![stream_id],
                )
                .unwrap();
        }

        let (durable_tx, _rx) = broadcast::channel(16);
        let registry: crate::retention::ProxyConsumerCursors = std::sync::Arc::default();
        let proxy = LocalProxy::bind_durable(
            0,
            stream_id.to_owned(),
            ReadSource::Mutex(db),
            durable_tx,
            registry.clone(),
        )
        .await
        .unwrap();

        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
            .await
            .unwrap();
        let mut buf = vec![0u8; b"threefour".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut buf),
        )
        .await
        .expect("retained rows must replay to a post-prune consumer")
        .unwrap();
        assert_eq!(&buf, b"threefour");

        // The consumer registered its cursor for the retention floor.
        assert_eq!(
            crate::retention::min_proxy_cursor(&registry, stream_id),
            Some(4)
        );
        proxy.shutdown();
    }

    #[tokio::test]
    async fn consumer_stalled_by_concurrent_prune_jumps_to_watermark() {
        // B1 regression: the consumer connects and reads watermark 0, then a
        // retention prune outruns it (rows 1..=4 deleted, watermark 4). The
        // drain must jump to the new watermark instead of stalling on the
        // head gap forever (which also wedged retention via the registered
        // cursor).
        let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
        let stream_id = "127.0.0.1:10700";
        let (durable_tx, rx) = broadcast::channel(16);
        let registry: crate::retention::ProxyConsumerCursors = std::sync::Arc::default();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (server_stream, _) = listener.accept().await.unwrap();
        let handle = tokio::spawn(serve_durable_consumer(
            server_stream,
            stream_id.to_owned(),
            ReadSource::Mutex(db.clone()),
            rx,
            registry.clone(),
        ));

        // Consumer is up with watermark 0 and an empty table. Now the prune
        // "wins the race": rows 5..=6 exist, 1..=4 are gone, watermark = 4.
        insert_durable_event(&db, stream_id, 5, b"five").await;
        insert_durable_event(&db, stream_id, 6, b"six").await;
        {
            let guard = db.lock().await;
            guard
                .raw_execute_for_test(
                    "INSERT INTO retention (stream_id, pruned_through_seq) VALUES (?1, 4)",
                    rusqlite::params![stream_id],
                )
                .unwrap();
        }
        durable_tx
            .send(crate::p2p_session::DurableBatch {
                through_seq: 6,
                inserted: std::sync::Arc::new(Vec::new()),
            })
            .unwrap();

        let mut buf = vec![0u8; b"fivesix".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut buf),
        )
        .await
        .expect("consumer must jump the pruned gap instead of stalling")
        .unwrap();
        assert_eq!(&buf, b"fivesix");

        // The registered cursor advanced past the watermark, so retention is
        // not wedged by this consumer.
        assert_eq!(
            crate::retention::min_proxy_cursor(&registry, stream_id),
            Some(6)
        );
        drop(durable_tx);
        handle.abort();
    }

    #[tokio::test]
    async fn consumer_jumps_p2p_gap_markers_like_the_durable_cursor() {
        // S2 regression: seqs 1..=14 were never stored (forwarder gap notice,
        // recorded as a gap marker). A consumer starting at 0 must jump to
        // earliest_available - 1 like the P2P cursor did, not stall forever
        // (and hold the retention floor at 0).
        let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
        let stream_id = "127.0.0.1:10800";
        insert_durable_event(&db, stream_id, 15, b"fifteen").await;
        insert_durable_event(&db, stream_id, 16, b"sixteen").await;
        {
            let guard = db.lock().await;
            guard
                .save_gap_marker(&crate::db::GapMarkerInsert {
                    stream_id,
                    requested_after_seq: 0,
                    earliest_available_seq: 15,
                    latest_available_seq: 16,
                    reason: "retention-window",
                    created_unix_ms: 1_700_000_000_000,
                })
                .unwrap();
        }

        let (durable_tx, _rx) = broadcast::channel(16);
        let proxy = LocalProxy::bind_durable(
            0,
            stream_id.to_owned(),
            ReadSource::Mutex(db),
            durable_tx,
            std::sync::Arc::default(),
        )
        .await
        .unwrap();

        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
            .await
            .unwrap();
        let mut buf = vec![0u8; b"fifteensixteen".len()];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut buf),
        )
        .await
        .expect("consumer must jump the recorded P2P gap")
        .unwrap();
        assert_eq!(&buf, b"fifteensixteen");
        proxy.shutdown();
    }

    #[tokio::test]
    async fn new_client_gets_replay() {
        let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
        // A real forwarder P2P stream_id (`ip:port`), not a parseable UUID.
        let stream_id = "127.0.0.1:10000";
        insert_durable_event(&db, stream_id, 2, b"second").await;
        insert_durable_event(&db, stream_id, 1, b"first").await;
        let (durable_tx, _rx) = broadcast::channel(16);
        let proxy = LocalProxy::bind_durable(
            0,
            stream_id.to_owned(),
            ReadSource::Mutex(db),
            durable_tx,
            std::sync::Arc::default(),
        )
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
        let proxy = LocalProxy::bind_durable(
            0,
            stream_id.to_owned(),
            ReadSource::Mutex(db.clone()),
            durable_tx.clone(),
            std::sync::Arc::default(),
        )
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
        durable_tx.send(hint(2)).unwrap();

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
        let proxy = LocalProxy::bind_durable(
            0,
            stream_id.to_owned(),
            ReadSource::Mutex(db.clone()),
            durable_tx.clone(),
            std::sync::Arc::default(),
        )
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
        durable_tx.send(hint(3)).unwrap();
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
        durable_tx.send(hint(2)).unwrap();

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
        durable_tx.send(hint(1)).unwrap();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (server_stream, _) = listener.accept().await.unwrap();
        let handle = tokio::spawn(serve_durable_consumer(
            server_stream,
            stream_id.to_owned(),
            ReadSource::Mutex(db),
            rx,
            std::sync::Arc::default(),
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
        let proxy = LocalProxy::bind_durable(
            0,
            stream_id.to_owned(),
            ReadSource::Mutex(db.clone()),
            durable_tx.clone(),
            std::sync::Arc::default(),
        )
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
        durable_tx.send(hint(2)).unwrap();
        durable_tx.send(hint(3)).unwrap();
        durable_tx.send(hint(4)).unwrap();

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
