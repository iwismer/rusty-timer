//! Integration tests for the headless P2P receiver runtime.
//!
//! These exercise the *real* loopback P2P lane end-to-end: a deterministic iroh
//! receiver endpoint dials a scripted [`MockForwarderPeer`], runs the real
//! per-forwarder P2P connection with data subscriptions, and drives the durable
//! local proxy, durable DBF feed, and server announcer push off post-commit
//! durable hints.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::routing::post;
use receiver::control_api::{ConnectionState, DiscoveredForwarder, DiscoveredStream};
use receiver::db::{DbfConfig, EventType, StreamSubscription};
use receiver::p2p_runtime::{
    ForwarderPeerConfig, P2pReceiverConfig, ReceiverIdentity, ServerClientConfig,
    start_receiver_p2p,
};
use receiver::ui_events::ReceiverUiEvent;
use rt_p2p_protocol::{
    EventBatch, Hello, MAX_FRAME_BYTES, ReadRecord, StreamCatalog, StreamEntry, SubscribeOk,
};
use rt_test_utils::p2p::{ConnectivityFault, ForwarderScript, MockForwarderPeer};
use rt_test_utils::poll_until;
use tokio::io::AsyncReadExt;

const STREAM_ID: &str = "127.0.0.1:10000";
const STREAM_ID_2: &str = "127.0.0.1:10001";
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
        echo_subscribed_stream_id: false,
        close_connection_after_data: false,
        control_events: Vec::new(),
        control_pings: 0,
        control_ping_interval: std::time::Duration::from_millis(50),
        config_get_json: String::new(),
        config_restart_needed: false,
        respond_to_config_requests: true,
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
        identity: ReceiverIdentity::Seed([seed; 32]),
        relay_disabled: true,
        discovery_disabled: true,
        bind_addr_v4: Some(std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::LOCALHOST,
            0,
        )),
        forwarder: Some(ForwarderPeerConfig {
            node_id,
            direct_addr: direct,
        }),
        server: None,
        reconcile_interval: Duration::from_millis(50),
    };
    (config, sub)
}

#[tokio::test]
async fn runtime_projects_canonical_stream_address_events_to_ui_state() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([68; 32], script_two(VALID_FRAME))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        let (config, mut sub) = base_config(node_id.clone(), direct, 69, None);
        sub.reader_ip = None;
        sub.forwarder_id = None;
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
                let node_id = node_id.clone();
                async move {
                    state.get_stream_metrics_snapshot().await.iter().any(|m| {
                        m.forwarder_id == node_id
                            && m.reader_ip == STREAM_ID
                            && m.raw_count == 2
                            && m.dedup_count == 2
                            && m.epoch_raw_count == 2
                            && m.unique_chips == 1
                    })
                }
            },
            Duration::from_secs(10),
        )
        .await;

        let streams = state.build_streams_response().await.streams;
        let stream = streams
            .iter()
            .find(|stream| stream.stream_id == STREAM_ID)
            .expect("subscribed stream present");
        assert_eq!(stream.forwarder_id.as_deref(), Some(node_id.as_str()));
        assert_eq!(stream.reader_ip.as_deref(), Some(STREAM_ID));
        assert_eq!(stream.reads_total, Some(2));
        assert_eq!(
            stream.local_port,
            receiver::ports::default_port(STREAM_ID),
            "canonical stream address should still get a default local proxy port"
        );

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("runtime_projects_canonical_stream_address_events_to_ui_state timed out");
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

// --- Mock server for announcer push -------------------------------------

#[derive(Clone, Default)]
struct ServerState {
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

async fn takeover_handler(State(state): State<ServerState>) -> Json<serde_json::Value> {
    let mut current = state.generation.lock().unwrap();
    *current += 1;
    Json(serde_json::json!({ "announcer_source_generation": *current }))
}

async fn rows_handler(
    State(state): State<ServerState>,
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

async fn start_mock_server() -> (String, ServerState) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = serve_mock_server(listener);
    (format!("http://{addr}"), state)
}

/// Bind and serve a mock server on a specific (already-reserved) port. Used
/// to prove the receiver recovers when the server appears *after* startup.
async fn start_mock_server_on_port(port: u16) -> ServerState {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
        .await
        .unwrap();
    serve_mock_server(listener)
}

fn serve_mock_server(listener: tokio::net::TcpListener) -> ServerState {
    let state = ServerState::default();
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
        let (thin_url, thin_state) = start_mock_server().await;

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        {
            let mut db = state.db.lock().await;
            db.replace_stream_subscriptions(&[stream_subscription(&node_id, None)])
                .unwrap();
            // Opt this stream in to announcer publishing (global + per-stream).
            db.set_announcer_enabled(true).unwrap();
            db.set_stream_announcer_publish(STREAM_ID, true).unwrap();
        }

        let (mut config, _sub) = base_config(node_id, direct, 77, None);
        config.server = Some(ServerClientConfig {
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

/// Phase 6 / Task 6.2: with the stream NOT opted in to announcer publishing,
/// the receiver still registers + takes over (generation acquired) but pushes
/// NO rows. Verifies per-stream gating.
#[tokio::test]
async fn announcer_does_not_push_when_stream_not_opted_in() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([88; 32], script_two(VALID_FRAME))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);
        let (thin_url, thin_state) = start_mock_server().await;

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        {
            let mut db = state.db.lock().await;
            db.replace_stream_subscriptions(&[stream_subscription(&node_id, None)])
                .unwrap();
            // Global toggle on, but the stream is NOT opted in.
            db.set_announcer_enabled(true).unwrap();
        }

        let (mut config, _sub) = base_config(node_id, direct, 89, None);
        config.server = Some(ServerClientConfig {
            url: thin_url,
            token: "secret-token".to_owned(),
        });
        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // Wait until events are durable and the generation has been taken over,
        // proving the server path ran end-to-end.
        poll_until(
            || {
                let state = Arc::clone(&state);
                let thin_state = thin_state.clone();
                async move {
                    let durable = {
                        let db = state.db.lock().await;
                        db.load_received_events(STREAM_ID)
                            .map(|e| e.len() >= 2)
                            .unwrap_or(false)
                    };
                    durable && *thin_state.generation.lock().unwrap() >= 1
                }
            },
            Duration::from_secs(10),
        )
        .await;

        // Give any (erroneous) push a chance to land, then assert none did.
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            thin_state.rows.lock().unwrap().is_empty(),
            "no rows must be pushed for a stream that is not opted in"
        );

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("announcer_does_not_push_when_stream_not_opted_in timed out");
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
/// records carry a *different* `stream_id`. The receiver data subscription
/// validates the record stream id and fails with a non-retryable
/// `StreamIdMismatch`, so the forwarder connection must resubscribe the desired
/// stream instead of leaving a finished data task in its task map.
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
        echo_subscribed_stream_id: false,
        close_connection_after_data: false,
        control_events: Vec::new(),
        control_pings: 0,
        control_ping_interval: std::time::Duration::from_millis(50),
        config_get_json: String::new(),
        config_restart_needed: false,
        respond_to_config_requests: true,
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

/// A non-retryable data subscription failure exits that stream's data task.
/// Reconciliation must keep the stream desired on the per-forwarder connection,
/// which resubscribes instead of leaving the finished task in place forever.
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

        // A lingering finished data task would never re-subscribe, so seeing
        // multiple subscribes proves the desired stream is resubscribed.
        poll_until(
            || async { forwarder.subscribes().len() >= 3 },
            Duration::from_secs(10),
        )
        .await;
        assert!(
            forwarder.subscribes().len() >= 3,
            "finished data task must be replaced, producing repeated subscribes"
        );

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("dead_session_worker_is_recreated_on_next_reconcile timed out");
}

/// The server is unavailable when the receiver starts, so register/takeover
/// fails and the announcer generation cannot be acquired. The reconcile loop
/// must keep retrying startup; once the server appears, a generation is
/// acquired, the stream worker is rebuilt with an announcer push worker, and the
/// pending durable rows are pushed — all without any new durable hint required.
#[tokio::test]
async fn announcer_push_recovers_when_server_starts_late() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([84; 32], script_two(VALID_FRAME))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        // Reserve a port for the server but do NOT start it yet, so initial
        // register/takeover fails with connection-refused.
        let thin_port = reserve_port().await;
        let thin_url = format!("http://127.0.0.1:{thin_port}");

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        {
            let mut db = state.db.lock().await;
            db.replace_stream_subscriptions(&[stream_subscription(&node_id, None)])
                .unwrap();
            db.set_announcer_enabled(true).unwrap();
            db.set_stream_announcer_publish(STREAM_ID, true).unwrap();
        }

        let (mut config, _sub) = base_config(node_id, direct, 85, None);
        config.server = Some(ServerClientConfig {
            url: thin_url,
            token: "secret-token".to_owned(),
        });

        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // Events become durable even while the server is unreachable.
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

        // Now bring the server up on the reserved port.
        let thin_state = start_mock_server_on_port(thin_port).await;

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
            "a generation must be taken over after the server recovers"
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
            "both pending rows pushed after server recovery"
        );

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("announcer_push_recovers_when_server_starts_late timed out");
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

/// Discovered-but-unsubscribed forwarder streams must surface in the streams
/// response as `subscribed = false` so the UI can list them, and a subscription
/// for the same (forwarder_endpoint_id, stream_id) must take precedence without
/// producing a duplicate entry.
#[tokio::test]
async fn discovered_streams_appear_as_unsubscribed_then_subscribed_dedup() {
    let dir = tempfile::tempdir().unwrap();
    let state = init_state(dir.path()).await;

    state.discovered_forwarders.write().await.insert(
        "fwd-endpoint-1".to_owned(),
        DiscoveredForwarder {
            display_name: Some("Start Line".to_owned()),
            direct_addrs: vec!["127.0.0.1:5000".parse().unwrap()],
            streams: vec![DiscoveredStream {
                stream_id: "reader-a".to_owned(),
                epoch: 2,
                next_seq: 5,
            }],
        },
    );

    let resp = state.build_streams_response().await;
    assert_eq!(resp.streams.len(), 1, "discovered stream is listed");
    let entry = &resp.streams[0];
    assert_eq!(entry.forwarder_endpoint_id, "fwd-endpoint-1");
    assert_eq!(entry.stream_id, "reader-a");
    assert!(
        !entry.subscribed,
        "discovered stream is available, not subscribed"
    );
    assert_eq!(entry.display_alias.as_deref(), Some("Start Line"));
    assert_eq!(entry.stream_epoch, Some(2));

    // Subscribe to the same stream: the subscribed entry replaces the discovered
    // one (dedupe by (forwarder_endpoint_id, stream_id)).
    state
        .db
        .lock()
        .await
        .replace_stream_subscriptions(&[StreamSubscription {
            forwarder_endpoint_id: "fwd-endpoint-1".to_owned(),
            stream_id: "reader-a".to_owned(),
            local_port_override: None,
            event_type: EventType::Finish,
            forwarder_id: None,
            reader_ip: None,
        }])
        .unwrap();

    let resp = state.build_streams_response().await;
    assert_eq!(
        resp.streams.len(),
        1,
        "subscribed entry dedupes the discovered one"
    );
    assert!(resp.streams[0].subscribed);
}

/// With no explicit forwarder config, a forwarder learned only from discovery
/// (injected into `discovered_forwarders`, as the server feed would) must be
/// dialed and its events persisted once a subscription names it.
#[tokio::test]
async fn discovered_forwarder_is_dialed_and_persists_events() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([92; 32], script_two(b"frame"))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;

        // Populate discovery WITHOUT any explicit forwarder config.
        state.discovered_forwarders.write().await.insert(
            node_id.clone(),
            DiscoveredForwarder {
                display_name: Some("Finish".to_owned()),
                direct_addrs: vec![direct],
                streams: vec![DiscoveredStream {
                    stream_id: STREAM_ID.to_owned(),
                    epoch: 1,
                    next_seq: 3,
                }],
            },
        );

        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[stream_subscription(&node_id, None)])
            .unwrap();

        let config = P2pReceiverConfig {
            identity: ReceiverIdentity::Seed([93; 32]),
            relay_disabled: true,
            discovery_disabled: true,
            bind_addr_v4: Some(std::net::SocketAddrV4::new(
                std::net::Ipv4Addr::LOCALHOST,
                0,
            )),
            forwarder: None,
            server: None,
            reconcile_interval: Duration::from_millis(50),
        };
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
        assert_eq!(seqs, vec![1, 2], "discovery-driven dial persisted events");
        drop(db);

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("discovered_forwarder_is_dialed_and_persists_events timed out");
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

/// Build a subscription for an arbitrary `stream_id` on a forwarder.
fn stream_subscription_for(forwarder_endpoint_id: &str, stream_id: &str) -> StreamSubscription {
    StreamSubscription {
        forwarder_endpoint_id: forwarder_endpoint_id.to_owned(),
        stream_id: stream_id.to_owned(),
        local_port_override: None,
        event_type: EventType::Finish,
        forwarder_id: None,
        reader_ip: Some(stream_id.to_owned()),
    }
}

/// Like [`script_two`] but echoes the subscribed `stream_id` into every outbound
/// data frame, so the one script can serve several distinct streams (each gets
/// records tagged with its own id) over a single connection.
fn script_two_echo(raw: &[u8]) -> ForwarderScript {
    let mut script = script_two(raw);
    script.echo_subscribed_stream_id = true;
    script
}

/// Two stream subscriptions to ONE forwarder must be served by a single control
/// session multiplexing two data streams over the SAME QUIC connection: both
/// deliver their records, only one connection is opened, and removing one
/// subscription leaves the other's durable rows and the control session intact.
///
/// Note: after the unsubscribe this asserts durable rows + control-session
/// survival (one connection, still `Connected`/`Subscribed`), NOT live-stream
/// liveness on the remaining stream — the scripted mock completes each data
/// subscription after its replay, so the harness cannot observe an ongoing live
/// stream here.
#[tokio::test]
async fn one_connection_multiplexes_multiple_data_streams() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let forwarder = MockForwarderPeer::start([92; 32], script_two_echo(VALID_FRAME))
            .await
            .unwrap();
        let (node_id, direct) = forwarder_config(&forwarder);

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;
        let (config, _sub) = base_config(node_id.clone(), direct, 93, None);
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[
                stream_subscription_for(&node_id, STREAM_ID),
                stream_subscription_for(&node_id, STREAM_ID_2),
            ])
            .unwrap();

        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // Both streams deliver their records durably.
        poll_until(
            || {
                let state = Arc::clone(&state);
                async move {
                    let db = state.db.lock().await;
                    let a = db
                        .load_received_events(STREAM_ID)
                        .map(|e| e.len())
                        .unwrap_or(0);
                    let b = db
                        .load_received_events(STREAM_ID_2)
                        .map(|e| e.len())
                        .unwrap_or(0);
                    a >= 2 && b >= 2
                }
            },
            Duration::from_secs(10),
        )
        .await;

        // Both subscriptions are served over ONE QUIC connection — one control
        // session multiplexing two data streams — not one connection per stream.
        assert_eq!(
            forwarder.connection_count(),
            1,
            "both data streams must share a single forwarder connection"
        );
        assert!(
            forwarder.subscribes().len() >= 2,
            "both streams must have subscribed over the shared connection"
        );

        // Remove one subscription. The other stream's durable rows and the
        // single control connection must remain intact (no reconnect).
        state
            .db
            .lock()
            .await
            .replace_stream_subscriptions(&[stream_subscription_for(&node_id, STREAM_ID)])
            .unwrap();

        // Let reconcile drop the removed stream worker; the remaining stream's
        // rows persist and no new connection is opened.
        poll_until(
            || {
                let state = Arc::clone(&state);
                async move {
                    let db = state.db.lock().await;
                    db.load_received_events(STREAM_ID)
                        .map(|e| e.len())
                        .unwrap_or(0)
                        >= 2
                }
            },
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(
            forwarder.connection_count(),
            1,
            "removing one subscription must not reconnect the control session"
        );

        // The control session is intact: the forwarder still reads as connected
        // (control up; the data replays have completed).
        let snapshot = state.forwarder_state(&node_id).await;
        assert!(
            matches!(
                snapshot.state,
                receiver::control_api::ForwarderConnState::Connected
                    | receiver::control_api::ForwarderConnState::Subscribed
            ),
            "control session must stay up after removing one subscription, got {:?}",
            snapshot.state
        );

        runtime.shutdown().await;
        forwarder.shutdown().await;
    })
    .await
    .expect("one_connection_multiplexes_multiple_data_streams timed out");
}

/// Phase 4 / Task 4.3: a runtime started with no server must rebind its
/// server-bound tasks (register -> takeover -> discovery) when the stored
/// profile gains a server and the `server_config_version` signal fires.
#[tokio::test]
async fn reconfigure_on_signal_rebinds_to_profile_server() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let (server_url, server_state) = start_mock_server().await;

        let dir = tempfile::tempdir().unwrap();
        let state = init_state(dir.path()).await;

        // Start a bare runtime with NO server configured.
        let config = P2pReceiverConfig {
            identity: ReceiverIdentity::Seed([71u8; 32]),
            relay_disabled: true,
            discovery_disabled: true,
            bind_addr_v4: Some("127.0.0.1:0".parse().unwrap()),
            forwarder: None,
            server: None,
            reconcile_interval: Duration::from_millis(50),
        };
        let runtime = start_receiver_p2p(Arc::clone(&state), config)
            .await
            .unwrap();

        // No server configured yet, so takeover must not have run.
        assert_eq!(*server_state.generation.lock().unwrap(), 0);

        // Save a profile pointing at the mock server.
        {
            let mut db = state.db.lock().await;
            db.save_profile(&server_url, "tok", "check-and-download", None)
                .unwrap();
        }

        // The reconcile loop must rebind and run register + takeover against the
        // newly-configured server. Re-signal on each poll so the test does not
        // race the spawned loop's watch subscription (production calls
        // notify_server_config_changed long after the loop subscribes, so the
        // edge-triggered signal is never missed there). Re-resolving to the
        // same server is idempotent (no extra rebind/takeover).
        poll_until(
            || {
                let server_state = server_state.clone();
                let state = Arc::clone(&state);
                async move {
                    state.notify_server_config_changed();
                    *server_state.generation.lock().unwrap() >= 1
                }
            },
            Duration::from_secs(10),
        )
        .await;

        tokio::time::timeout(Duration::from_secs(3), runtime.shutdown())
            .await
            .expect("runtime shutdown must complete promptly");
    })
    .await
    .expect("reconfigure_on_signal_rebinds_to_profile_server timed out");
}
