//! Behavioral coverage for the durable local proxy (`LocalProxy::bind_durable`).
//!
//! These tests are written race-free: the proxy binds port 0 and the tests
//! dial `proxy.port`, and delivery is asserted with bounded `read_exact`
//! timeouts instead of fixed sleeps. Correctness does not depend on when the
//! accept loop runs relative to a durable hint: rows inserted before the hint
//! are always visible to a consumer's initial durable drain.

use receiver::db::{Db, ReceivedEventInsert};
use receiver::local_proxy::LocalProxy;
use receiver::p2p_session::DurableBatch;
use receiver::read_pool::ReadSource;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::{Mutex, broadcast};

fn hint(through_seq: i64) -> DurableBatch {
    DurableBatch {
        through_seq,
        inserted: Arc::new(Vec::new()),
    }
}

async fn insert_durable_event(db: &Arc<Mutex<Db>>, stream_id: &str, seq: i64, raw_frame: &[u8]) {
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

async fn bind_proxy(
    db: &Arc<Mutex<Db>>,
    stream_id: &str,
) -> (LocalProxy, broadcast::Sender<DurableBatch>) {
    let (durable_tx, _rx) = broadcast::channel(16);
    let proxy = LocalProxy::bind_durable(
        0,
        stream_id.to_owned(),
        ReadSource::Mutex(Arc::clone(db)),
        durable_tx.clone(),
        Arc::default(),
    )
    .await
    .expect("bind should succeed");
    (proxy, durable_tx)
}

async fn read_exact_bytes(client: &mut tokio::net::TcpStream, expected: &[u8], what: &str) {
    let mut buf = vec![0u8; expected.len()];
    tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut buf))
        .await
        .unwrap_or_else(|_| panic!("{what} timed out"))
        .unwrap();
    assert_eq!(buf, expected, "{what}: bytes must be exact");
}

/// The proxy port is open and connectable before any events exist, and
/// multiple events arriving afterwards are delivered contiguously in seq
/// order with exact bytes.
#[tokio::test]
async fn durable_proxy_binds_before_events_and_delivers_in_sequence() {
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let stream_id = "127.0.0.1:12000";
    let (proxy, durable_tx) = bind_proxy(&db, stream_id).await;

    let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
        .await
        .expect("should connect before any events arrive");

    insert_durable_event(&db, stream_id, 1, b"one").await;
    insert_durable_event(&db, stream_id, 2, b"two").await;
    insert_durable_event(&db, stream_id, 3, b"three").await;
    // A single hint after all inserts: whether the consumer sees the hint or
    // drains the rows on connect, the durable store already holds them.
    let _ = durable_tx.send(hint(3));

    read_exact_bytes(&mut client, b"onetwothree", "in-sequence delivery").await;
    proxy.shutdown();
}

/// Every simultaneously connected consumer receives both the durable replay
/// and subsequent live events.
#[tokio::test]
async fn durable_proxy_multiple_consumers_all_receive() {
    let db = Arc::new(Mutex::new(Db::open_in_memory().unwrap()));
    let stream_id = "127.0.0.1:12001";
    insert_durable_event(&db, stream_id, 1, b"alpha").await;
    let (proxy, durable_tx) = bind_proxy(&db, stream_id).await;

    let mut consumers = Vec::new();
    for i in 0..3 {
        let mut client = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", proxy.port))
            .await
            .unwrap();
        read_exact_bytes(&mut client, b"alpha", &format!("consumer {i} replay")).await;
        consumers.push(client);
    }

    insert_durable_event(&db, stream_id, 2, b"beta").await;
    let _ = durable_tx.send(hint(2));

    for (i, client) in consumers.iter_mut().enumerate() {
        read_exact_bytes(client, b"beta", &format!("consumer {i} live event")).await;
    }
    proxy.shutdown();
}
