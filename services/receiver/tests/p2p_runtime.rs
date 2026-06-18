//! Integration tests for the headless P2P receiver runtime.
//!
//! These exercise the *real* loopback P2P lane end-to-end: a deterministic iroh
//! receiver endpoint dials a scripted [`MockForwarderPeer`], runs the real
//! reconnecting `p2p_session`, and drives the durable local proxy, durable DBF
//! feed, and thin-node announcer push off the post-commit durable hints.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::routing::post;
use receiver::control_api::ConnectionState;
use receiver::db::{DbfConfig, EventType, StreamSubscription};
use receiver::p2p_runtime::{
    ForwarderPeerConfig, P2pReceiverConfig, ThinNodeClientConfig, start_receiver_p2p,
};
use receiver::ui_events::ReceiverUiEvent;
use rt_p2p_protocol::{
    EventBatch, Hello, MAX_FRAME_BYTES, ReadRecord, StreamCatalog, StreamEntry, SubscribeOk,
};
use rt_test_utils::p2p::{ConnectivityFault, ForwarderScript, MockForwarderPeer};
use rt_test_utils::poll_until;
use tokio::io::AsyncReadExt;

const STREAM_ID: &str = "127.0.0.1:10000";
// A valid IPICO chip-read frame (chip id `000000012345`) so DBF/announcer
// mapping produces real output.
const VALID_FRAME: &[u8] = b"aa400000000123450a2a01123018455927a7";
const TEST_TIMEOUT: Duration = Duration::from_secs(20);

fn sid_bytes() -> Vec<u8> {
    STREAM_ID.as_bytes().to_vec()
}

fn server_hello() -> Hello {
    Hello {
        min_minor: 1,
        max_minor: 1,
        capabilities: vec!["data".to_owned()],
        max_frame_bytes: u32::try_from(MAX_FRAME_BYTES).unwrap(),
        catalog_generation: 1,
    }
}

fn catalog() -> StreamCatalog {
    StreamCatalog {
        generation: 1,
        entries: vec![StreamEntry {
            stream_id: sid_bytes(),
            display_name: "Finish".to_owned(),
            network_addr: "10.0.0.1:10000".to_owned(),
            reader_connected: true,
            hardware_reader_id: "R1".to_owned(),
        }],
    }
}

fn record(seq: u64, raw: &[u8]) -> ReadRecord {
    ReadRecord {
        stream_id: sid_bytes(),
        seq,
        epoch: 1,
        raw_frame: raw.to_vec(),
        read_kind: "chip".to_owned(),
        reader_timestamp: 0,
        received_unix_ms: 1_700_000_000_000 + i64::try_from(seq).unwrap(),
    }
}

/// A script that serves `[1, 2]` with `raw` frames and a CaughtUp through 2.
fn script_two(raw: &[u8]) -> ForwarderScript {
    ForwarderScript {
        server_hello: server_hello(),
        catalog: catalog(),
        subscribe_ok: SubscribeOk {
            stream_id: sid_bytes(),
            earliest_available_seq: 1,
            latest_seq_at_open: 2,
        },
        gap_notice: None,
        batches: vec![EventBatch {
            records: vec![record(1, raw), record(2, raw)],
            replay: false,
        }],
        caught_up_through: Some(2),
        data_fault: ConnectivityFault::healthy(),
    }
}

async fn init_state(data_dir: &std::path::Path) -> Arc<receiver::control_api::AppState> {
    let (state, _shutdown_rx) = receiver::runtime::init_with_data_dir(None, data_dir)
        .await
        .expect("init receiver state");
    state
}

fn stream_subscription(
    forwarder_endpoint_id: &str,
    local_port_override: Option<u16>,
) -> StreamSubscription {
    StreamSubscription {
        forwarder_endpoint_id: forwarder_endpoint_id.to_owned(),
        stream_id: STREAM_ID.to_owned(),
        local_port_override,
        event_type: EventType::Finish,
        forwarder_id: None,
        reader_ip: Some(STREAM_ID.to_owned()),
    }
}

fn forwarder_config(forwarder: &MockForwarderPeer) -> (String, SocketAddr) {
    let addr = forwarder.node_addr();
    let node_id = addr.node_id.to_string();
    let direct = *addr
        .direct_addresses
        .iter()
        .next()
        .expect("forwarder direct address");
    (node_id, direct)
}

fn base_config(
    node_id: String,
    direct: SocketAddr,
    seed: u8,
    local_port_override: Option<u16>,
) -> (P2pReceiverConfig, StreamSubscription) {
    let sub = stream_subscription(&node_id, local_port_override);
    let config = P2pReceiverConfig {
        secret_key_seed: [seed; 32],
        forwarder: ForwarderPeerConfig {
            node_id,
            direct_addr: direct,
        },
        thin_node: None,
        reconcile_interval: Duration::from_millis(50),
    };
    (config, sub)
}

#[tokio::test]
async fn runtime_persists_events_and_advances_cursor() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([70; 32], script_two(b"frame"))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        let (config, sub) = base_config(node_id, direct, 71, None);
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[sub])
            .unwrap();

        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        poll_until(
            || {
                let state = Arc::clone(&state);
                async move {
                    let db = state.db.lock().await;
                    db.load_received_events(STREAM_ID)
                        .map(|e| e.len() >= 2)
                        .unwrap_or(false)
                }
            },
            Duration::from_secs(10),
        )
        .await;

        let db = state.db.lock().await;
        let seqs: Vec<i64> = db
            .load_received_events(STREAM_ID)
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect();
        assert_eq!(seqs, vec![1, 2], "exact durable rows");
        assert_eq!(
            db.load_stream_cursor(STREAM_ID).unwrap(),
            2,
            "cursor advanced over the durable contiguous prefix"
        );
        drop(db);

        // Ack-after-durable: the forwarder only ever sees acks through the
        // durable cursor.
        poll_until(
            || async { forwarder.acks().iter().any(|a| a.through_seq == 2) },
            Duration::from_secs(5),
        )
        .await;
        assert!(forwarder.acks().iter().all(|a| a.through_seq <= 2));

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("runtime_persists_events_and_advances_cursor timed out");
}

#[tokio::test]
async fn durable_local_proxy_replays_exact_frames() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([72; 32], script_two(b"frame"))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        // Reserve an ephemeral port for the durable proxy so the test can dial it.
        let port = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port
        };

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        let (config, sub) = base_config(node_id, direct, 73, Some(port));
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[sub])
            .unwrap();

        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // Wait until events are durable.
        poll_until(
            || {
                let state = Arc::clone(&state);
                async move {
                    let db = state.db.lock().await;
                    db.load_received_events(STREAM_ID)
                        .map(|e| e.len() >= 2)
                        .unwrap_or(false)
                }
            },
            Duration::from_secs(10),
        )
        .await;

        // Connect a fresh consumer; it must replay the exact raw frames in order.
        let mut client = poll_connect(port).await;
        let mut buf = vec![0u8; b"frameframe".len()];
        tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut buf))
            .await
            .expect("proxy replay should not time out")
            .unwrap();
        assert_eq!(&buf, b"frameframe", "exact raw frame replay");

        // A second consumer reconnecting gets the same replay with no duplicate
        // frames (durable store is the source of truth).
        let mut client2 = poll_connect(port).await;
        let mut buf2 = vec![0u8; b"frameframe".len()];
        tokio::time::timeout(Duration::from_secs(5), client2.read_exact(&mut buf2))
            .await
            .expect("second proxy replay should not time out")
            .unwrap();
        assert_eq!(&buf2, b"frameframe");
        // No extra bytes beyond the two durable frames.
        let mut extra = [0u8; 1];
        assert!(
            tokio::time::timeout(Duration::from_millis(200), client2.read_exact(&mut extra))
                .await
                .is_err(),
            "no duplicate frames should be delivered on reconnect"
        );

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("durable_local_proxy_replays_exact_frames timed out");
}

async fn poll_connect(port: u16) -> tokio::net::TcpStream {
    for _ in 0..100 {
        if let Ok(stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            return stream;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("could not connect to durable proxy on port {port}");
}

#[tokio::test]
async fn dbf_feed_delivers_from_received_events_without_duplicates() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([74; 32], script_two(VALID_FRAME))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        let dir = tempfile::tempdir().unwrap();
        let dbf_path = dir.path().join("out.dbf");
        let state = init_state(dir.path()).await;
        {
            let mut db = state.db.lock().await;
            db.save_dbf_config(&DbfConfig {
                enabled: true,
                path: dbf_path.to_string_lossy().into_owned(),
            })
            .unwrap();
            db.replace_stream_subscriptions(&[stream_subscription(&node_id, None)])
                .unwrap();
        }

        let (config, _sub) = base_config(node_id, direct, 75, None);
        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // Wait until both events are durable AND marked DBF-delivered.
        poll_until(
            || {
                let state = Arc::clone(&state);
                async move {
                    let db = state.db.lock().await;
                    let e1 = db.load_received_event(STREAM_ID, 1).ok().flatten();
                    let e2 = db.load_received_event(STREAM_ID, 2).ok().flatten();
                    matches!((e1, e2), (Some(a), Some(b))
                        if a.dbf_delivered_unix_ms.is_some()
                        && b.dbf_delivered_unix_ms.is_some())
                }
            },
            Duration::from_secs(10),
        )
        .await;

        assert!(dbf_path.exists(), "DBF file written from received_events");

        // Snapshot the delivery markers, let the runtime keep reconnecting and
        // re-running the DBF feed, then prove no row was re-delivered.
        let markers_before: Vec<Option<i64>> = {
            let db = state.db.lock().await;
            vec![
                db.load_received_event(STREAM_ID, 1)
                    .unwrap()
                    .unwrap()
                    .dbf_delivered_unix_ms,
                db.load_received_event(STREAM_ID, 2)
                    .unwrap()
                    .unwrap()
                    .dbf_delivered_unix_ms,
            ]
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        let markers_after: Vec<Option<i64>> = {
            let db = state.db.lock().await;
            vec![
                db.load_received_event(STREAM_ID, 1)
                    .unwrap()
                    .unwrap()
                    .dbf_delivered_unix_ms,
                db.load_received_event(STREAM_ID, 2)
                    .unwrap()
                    .unwrap()
                    .dbf_delivered_unix_ms,
            ]
        };
        assert_eq!(
            markers_before, markers_after,
            "DBF delivery markers must not change on replay/retry (no duplicate delivery)"
        );

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("dbf_feed_delivers_from_received_events_without_duplicates timed out");
}

// --- Mock thin-node for announcer push -------------------------------------

#[derive(Clone, Default)]
struct ThinNodeState {
    rows: Arc<Mutex<Vec<(String, u64, u64)>>>, // (stream_id, seq, generation)
    generation: Arc<Mutex<u64>>,
}

async fn register_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "endpoint_id": "ep",
        "device_kind": "receiver",
        "approval_state": "active"
    }))
}

async fn takeover_handler(State(state): State<ThinNodeState>) -> Json<serde_json::Value> {
    let mut current = state.generation.lock().unwrap();
    *current += 1;
    Json(serde_json::json!({ "announcer_source_generation": *current }))
}

async fn rows_handler(
    State(state): State<ThinNodeState>,
    Json(body): Json<serde_json::Value>,
) -> Json<serde_json::Value> {
    let stream_id = body["stream_id"].as_str().unwrap_or_default().to_owned();
    let seq = body["seq"].as_u64().unwrap_or_default();
    let generation = body["announcer_source_generation"]
        .as_u64()
        .unwrap_or_default();
    state
        .rows
        .lock()
        .unwrap()
        .push((stream_id, seq, generation));
    Json(serde_json::json!({ "announcer_source_generation": generation, "finisher_count": 1 }))
}

async fn start_mock_thin_node() -> (String, ThinNodeState) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = serve_mock_thin_node(listener);
    (format!("http://{addr}"), state)
}

/// Bind and serve a mock thin-node on a specific (already-reserved) port. Used
/// to prove the receiver recovers when the thin-node appears *after* startup.
async fn start_mock_thin_node_on_port(port: u16) -> ThinNodeState {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    serve_mock_thin_node(listener)
}

fn serve_mock_thin_node(listener: tokio::net::TcpListener) -> ThinNodeState {
    let state = ThinNodeState::default();
    let app = axum::Router::new()
        .route("/register", post(register_handler))
        .route("/announcer/takeover", post(takeover_handler))
        .route("/announcer/rows", post(rows_handler))
        .with_state(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    state
}

#[tokio::test]
async fn announcer_push_pushes_rows_with_generation_and_no_duplicates() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([76; 32], script_two(VALID_FRAME))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);
        let (thin_url, thin_state) = start_mock_thin_node().await;

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[stream_subscription(&node_id, None)])
            .unwrap();

        let (mut config, _sub) = base_config(node_id, direct, 77, None);
        config.thin_node = Some(ThinNodeClientConfig {
            url: thin_url,
            token: "secret-token".to_owned(),
        });

        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // Wait until both rows have been pushed.
        poll_until(
            || {
                let rows = Arc::clone(&thin_state.rows);
                async move { rows.lock().unwrap().len() >= 2 }
            },
            Duration::from_secs(10),
        )
        .await;

        // Let the runtime keep reconnecting/re-hinting, then assert idempotency.
        tokio::time::sleep(Duration::from_millis(300)).await;
        let rows = thin_state.rows.lock().unwrap().clone();
        let generation = *thin_state.generation.lock().unwrap();
        assert_eq!(generation, 1, "takeover should be called exactly once");

        let mut keys: Vec<(String, u64)> =
            rows.iter().map(|(s, seq, _)| (s.clone(), *seq)).collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![(STREAM_ID.to_owned(), 1), (STREAM_ID.to_owned(), 2)],
            "each (stream_id, seq) pushed exactly once with no duplicate repush"
        );
        for (_, _, row_gen) in &rows {
            assert_eq!(*row_gen, 1, "rows fenced to the taken-over generation");
        }

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("announcer_push_pushes_rows_with_generation_and_no_duplicates timed out");
}

/// A record carrying an explicit `stream_id` (used to script a stream-id
/// mismatch the session must reject non-retryably).
fn record_with_sid(stream_id: &[u8], seq: u64, raw: &[u8]) -> ReadRecord {
    ReadRecord {
        stream_id: stream_id.to_vec(),
        seq,
        epoch: 1,
        raw_frame: raw.to_vec(),
        read_kind: "chip".to_owned(),
        reader_timestamp: 0,
        received_unix_ms: 1_700_000_000_000 + i64::try_from(seq).unwrap(),
    }
}

/// A script whose `SubscribeOk` is for the subscribed stream but whose batch
/// records carry a *different* `stream_id`. The receiver session validates the
/// record stream id and fails with a non-retryable `StreamIdMismatch`, so its
/// session task exits (no self-retry) — exactly the condition that would leave a
/// dead worker in the reconcile map without a rebuild-on-finish pass.
fn script_stream_mismatch() -> ForwarderScript {
    let other = b"127.0.0.1:10001".to_vec();
    ForwarderScript {
        server_hello: server_hello(),
        catalog: catalog(),
        subscribe_ok: SubscribeOk {
            stream_id: sid_bytes(),
            earliest_available_seq: 1,
            latest_seq_at_open: 1,
        },
        gap_notice: None,
        batches: vec![EventBatch {
            records: vec![record_with_sid(&other, 1, b"frame")],
            replay: false,
        }],
        caught_up_through: None,
        data_fault: ConnectivityFault::healthy(),
    }
}

/// Reserve an ephemeral loopback TCP port and release it so a proxy can bind it.
async fn reserve_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Changing a live subscription's `local_port_override` must tear down the old
/// stream worker (closing the old proxy port) and rebuild it on the new port,
/// which serves the durable replay. Reconciliation that only matched workers by
/// `stream_id` would silently keep the stale proxy on the old port.
#[tokio::test]
async fn changing_local_port_rebinds_proxy_to_new_port() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([80; 32], script_two(b"frame"))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        let old_port = reserve_port().await;
        let new_port = reserve_port().await;

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        let (config, sub) = base_config(node_id.clone(), direct, 81, Some(old_port));
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[sub])
            .unwrap();

        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // The old proxy serves the durable replay.
        let mut client = poll_connect(old_port).await;
        let mut buf = vec![0u8; b"frameframe".len()];
        tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut buf))
            .await
            .expect("old proxy replay should not time out")
            .unwrap();
        assert_eq!(&buf, b"frameframe", "old port replays exact frames");

        // Repoint the canonical subscription to the new local port.
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[stream_subscription(&node_id, Some(new_port))])
            .unwrap();

        // The new port must come up and serve the durable replay.
        let mut client_new = poll_connect(new_port).await;
        let mut buf_new = vec![0u8; b"frameframe".len()];
        tokio::time::timeout(Duration::from_secs(5), client_new.read_exact(&mut buf_new))
            .await
            .expect("new proxy replay should not time out")
            .unwrap();
        assert_eq!(&buf_new, b"frameframe", "new port replays exact frames");

        // The old port's listener must be gone: a fresh connection is refused
        // (or, if accepted during a race, yields an immediate EOF).
        poll_until(
            || async move {
                match tokio::net::TcpStream::connect(("127.0.0.1", old_port)).await {
                    Err(_) => true,
                    Ok(mut stream) => {
                        let mut byte = [0u8; 1];
                        matches!(
                            tokio::time::timeout(
                                Duration::from_millis(200),
                                stream.read_exact(&mut byte),
                            )
                            .await,
                            Ok(Err(_))
                        )
                    }
                }
            },
            Duration::from_secs(5),
        )
        .await;

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("changing_local_port_rebinds_proxy_to_new_port timed out");
}

/// A non-retryable session failure exits the session task. Reconciliation must
/// notice the finished session and rebuild the worker, which redials and
/// re-subscribes; without that, the dead worker would linger and the forwarder
/// would see exactly one subscribe forever.
#[tokio::test]
async fn dead_session_worker_is_recreated_on_next_reconcile() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([82; 32], script_stream_mismatch())
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        let (config, sub) = base_config(node_id, direct, 83, None);
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[sub])
            .unwrap();

        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // Each rebuilt worker redials and re-subscribes. A lingering dead worker
        // would never re-subscribe, so seeing multiple subscribes proves the
        // finished session was detected and the worker recreated.
        poll_until(
            || async { forwarder.subscribes().len() >= 3 },
            Duration::from_secs(10),
        )
        .await;
        assert!(
            forwarder.subscribes().len() >= 3,
            "dead worker must be recreated, producing repeated subscribes"
        );

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("dead_session_worker_is_recreated_on_next_reconcile timed out");
}

/// The thin-node is unavailable when the receiver starts, so register/takeover
/// fails and the announcer generation cannot be acquired. The reconcile loop
/// must keep retrying startup; once the thin-node appears, a generation is
/// acquired, the stream worker is rebuilt with an announcer push worker, and the
/// pending durable rows are pushed — all without any new durable hint required.
#[tokio::test]
async fn announcer_push_recovers_when_thin_node_starts_late() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([84; 32], script_two(VALID_FRAME))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        // Reserve a port for the thin-node but do NOT start it yet, so initial
        // register/takeover fails with connection-refused.
        let thin_port = reserve_port().await;
        let thin_url = format!("http://127.0.0.1:{thin_port}");

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[stream_subscription(&node_id, None)])
            .unwrap();

        let (mut config, _sub) = base_config(node_id, direct, 85, None);
        config.thin_node = Some(ThinNodeClientConfig {
            url: thin_url,
            token: "secret-token".to_owned(),
        });

        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // Events become durable even while the thin-node is unreachable.
        poll_until(
            || {
                let state = Arc::clone(&state);
                async move {
                    let db = state.db.lock().await;
                    db.load_received_events(STREAM_ID)
                        .map(|e| e.len() >= 2)
                        .unwrap_or(false)
                }
            },
            Duration::from_secs(10),
        )
        .await;

        // Now bring the thin-node up on the reserved port.
        let thin_state = start_mock_thin_node_on_port(thin_port).await;

        // The reconcile loop retries startup, acquires a generation, rebuilds
        // the worker, and pushes the pending rows.
        poll_until(
            || {
                let rows = Arc::clone(&thin_state.rows);
                async move { rows.lock().unwrap().len() >= 2 }
            },
            Duration::from_secs(10),
        )
        .await;

        let generation = *thin_state.generation.lock().unwrap();
        assert!(
            generation >= 1,
            "a generation must be taken over after the thin-node recovers"
        );
        let mut keys: Vec<(String, u64)> = thin_state
            .rows
            .lock()
            .unwrap()
            .iter()
            .map(|(s, seq, _)| (s.clone(), *seq))
            .collect();
        keys.sort();
        keys.dedup();
        assert_eq!(
            keys,
            vec![(STREAM_ID.to_owned(), 1), (STREAM_ID.to_owned(), 2)],
            "both pending rows pushed after thin-node recovery"
        );

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("announcer_push_recovers_when_thin_node_starts_late timed out");
}

/// A reconcile interval below the minimum must be rejected by
/// [`start_receiver_p2p`] rather than hot-polling.
#[tokio::test]
async fn start_receiver_p2p_rejects_tiny_reconcile_interval() {
    let forwarder = MockForwarderPeer::start([88; 32], script_two(b"frame"))
        .await
        .unwrap();
    let (node_id, direct) = forwarder_config(&forwarder);

    let dir = tempfile::tempdir().unwrap();
    let state = init_state(dir.path()).await;
    let (mut config, _sub) = base_config(node_id, direct, 89, None);
    config.reconcile_interval = Duration::from_millis(0);

    let err = match start_receiver_p2p(Arc::clone(&state), config).await {
        Ok(_) => panic!("zero reconcile interval must be rejected"),
        Err(err) => err,
    };
    assert!(
        err.contains("reconcile interval") && err.contains("minimum"),
        "got: {err}"
    );

    forwarder.shutdown().await;
}

/// The receiver's `connection_state` must reflect the real P2P session
/// lifecycle: once a session connects on loopback it reaches `Connected`, and
/// once the runtime is shut down it returns to `Disconnected`.
#[tokio::test]
async fn connection_state_reflects_p2p_session_lifecycle() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([90; 32], script_two(b"frame"))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        let (config, sub) = base_config(node_id, direct, 91, None);
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[sub])
            .unwrap();

        // Subscribe to status events before starting so the (non-coalesced)
        // broadcast stream reliably surfaces the Connected transition even
        // though the scripted mock closes each connection right after its ack
        // (which makes the live state briefly flap Connected -> Connecting).
        let mut ui_rx = state.ui_tx.subscribe();
        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // The live P2P session drives the connection state to Connected.
        let reached_connected = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match ui_rx.recv().await {
                    Ok(ReceiverUiEvent::StatusChanged {
                        connection_state: ConnectionState::Connected,
                        ..
                    }) => break,
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .await;
        assert!(
            reached_connected.is_ok(),
            "a live P2P session must drive the connection state to Connected"
        );

        // Shutting down the runtime returns the connection state to Disconnected.
        runtime.shutdown().await;
        assert_eq!(
            *state.conn_rx().borrow(),
            ConnectionState::Disconnected,
            "runtime shutdown must report Disconnected"
        );

        forwarder.shutdown().await;
    })
    .await
    .expect("connection_state_reflects_p2p_session_lifecycle timed out");
}

#[tokio::test]
async fn shutdown_cancels_runtime_promptly() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([78; 32], script_two(b"frame"))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[stream_subscription(&node_id, None)])
            .unwrap();

        let (config, _sub) = base_config(node_id, direct, 79, None);
        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // Let at least one session run.
        poll_until(
            || {
                let state = Arc::clone(&state);
                async move {
                    let db = state.db.lock().await;
                    db.load_received_events(STREAM_ID)
                        .map(|e| !e.is_empty())
                        .unwrap_or(false)
                }
            },
            Duration::from_secs(10),
        )
        .await;

        tokio::time::timeout(Duration::from_secs(3), runtime.shutdown())
            .await
            .expect("p2p runtime shutdown must complete promptly");

        forwarder.shutdown().await;
    })
    .await
    .expect("shutdown_cancels_runtime_promptly timed out");
}
