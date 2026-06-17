use axum::Router;
use axum::extract::State;
use axum::extract::ws::{WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use receiver::headless::{HeadlessConfig, HeadlessHost};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

fn loopback_ephemeral_config(data_dir: &std::path::Path) -> HeadlessConfig {
    HeadlessConfig {
        data_dir: data_dir.to_path_buf(),
        bind_addr: "127.0.0.1:0".parse().expect("parse bind addr"),
        receiver_id: None,
    }
}

#[tokio::test]
async fn boots_with_temp_data_dir() {
    let dir = tempfile::tempdir().expect("temp dir");
    let host = HeadlessHost::start(loopback_ephemeral_config(dir.path()))
        .await
        .expect("start headless host");

    let db_path = dir.path().join("receiver.sqlite3");
    assert!(db_path.exists(), "receiver DB must be created in data dir");

    let db = receiver::Db::open(&db_path).expect("open receiver DB from data dir");
    let profile = db
        .load_profile()
        .expect("load profile")
        .expect("profile row");
    assert!(
        profile
            .receiver_id
            .as_deref()
            .is_some_and(|id| id.starts_with("recv-")),
        "generated receiver id should be persisted in the data-dir DB"
    );

    host.shutdown().await.expect("shutdown headless host");
}

#[tokio::test]
async fn serves_get_status() {
    let dir = tempfile::tempdir().expect("temp dir");
    let host = HeadlessHost::start(loopback_ephemeral_config(dir.path()))
        .await
        .expect("start headless host");

    let response = reqwest::get(format!("http://{}/api/v1/status", host.local_addr()))
        .await
        .expect("GET /api/v1/status");
    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await.expect("status json");
    assert_eq!(body["connection_state"], "disconnected");
    assert_eq!(body["local_ok"], true);

    host.shutdown().await.expect("shutdown headless host");
}

/// In the default (no-feature) build the test bridge must not exist: the
/// headless host serves no `/bridge/*` routes.
#[cfg(not(feature = "test-bridge"))]
#[tokio::test]
async fn bridge_absent_without_feature() {
    let dir = tempfile::tempdir().expect("temp dir");
    let host = HeadlessHost::start(loopback_ephemeral_config(dir.path()))
        .await
        .expect("start headless host");

    let base = format!("http://{}", host.local_addr());
    let client = reqwest::Client::new();

    let state = client
        .get(format!("{base}/bridge/state"))
        .send()
        .await
        .expect("GET /bridge/state");
    assert_eq!(
        state.status().as_u16(),
        404,
        "/bridge/state must be absent without the test-bridge feature"
    );

    let invoke = client
        .post(format!("{base}/bridge/invoke/get_status"))
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("POST /bridge/invoke/get_status");
    assert_eq!(
        invoke.status().as_u16(),
        404,
        "/bridge/invoke must be absent without the test-bridge feature"
    );

    host.shutdown().await.expect("shutdown headless host");
}

/// With the `test-bridge` feature enabled the headless host serves the bridge
/// surface over its loopback control API.
#[cfg(feature = "test-bridge")]
#[tokio::test]
async fn bridge_present_with_feature() {
    let dir = tempfile::tempdir().expect("temp dir");
    let host = HeadlessHost::start(loopback_ephemeral_config(dir.path()))
        .await
        .expect("start headless host");

    let base = format!("http://{}", host.local_addr());
    let resp = reqwest::get(format!("{base}/bridge/state"))
        .await
        .expect("GET /bridge/state");
    assert!(resp.status().is_success(), "status: {}", resp.status());
    let body: serde_json::Value = resp.json().await.expect("state json");
    assert_eq!(body["status"]["connection_state"], "disconnected");
    assert!(body["streams"]["streams"].is_array());

    host.shutdown().await.expect("shutdown headless host");
}

#[tokio::test]
async fn rejects_non_loopback_bind_addr() {
    let dir = tempfile::tempdir().expect("temp dir");
    let config = HeadlessConfig {
        data_dir: dir.path().to_path_buf(),
        bind_addr: "0.0.0.0:0".parse().expect("parse bind addr"),
        receiver_id: None,
    };

    let err = match HeadlessHost::start(config).await {
        Ok(_) => panic!("non-loopback bind address must be rejected"),
        Err(err) => err,
    };
    assert!(
        err.contains("loopback"),
        "error should mention loopback requirement, got: {err}"
    );

    let db_path = dir.path().join("receiver.sqlite3");
    assert!(
        !db_path.exists(),
        "rejection must happen before DB initialization"
    );
}

#[tokio::test]
async fn clean_shutdown() {
    let dir = tempfile::tempdir().expect("temp dir");
    let host = HeadlessHost::start(loopback_ephemeral_config(dir.path()))
        .await
        .expect("start headless host");
    let addr = host.local_addr();

    tokio::time::timeout(Duration::from_secs(2), host.shutdown())
        .await
        .expect("shutdown should complete promptly")
        .expect("shutdown result");

    let rebound = tokio::net::TcpListener::bind(addr)
        .await
        .expect("control API port should be released after shutdown");
    drop(rebound);
}

/// Server state that signals (once) when the WebSocket upgrade completes.
#[derive(Clone)]
struct StallState {
    upgraded_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
}

/// Completes the WebSocket upgrade but never sends a heartbeat, so the
/// receiver's `do_handshake` would block forever on `ws.next()` without the
/// shutdown race.
async fn stalling_handler(ws: WebSocketUpgrade, State(st): State<StallState>) -> impl IntoResponse {
    ws.on_upgrade(move |mut socket: WebSocket| async move {
        // Wait for the receiver's hello before signaling. Once the server has
        // received it, the receiver has entered `do_handshake` and is about to
        // park on `ws.next()`, making the stall deterministic without a sleep.
        let _ = socket.recv().await;
        if let Some(tx) = st.upgraded_tx.lock().expect("lock upgraded_tx").take() {
            let _ = tx.send(());
        }
        // Drain further client messages without ever responding. The receiver
        // waits indefinitely for a heartbeat that never arrives.
        while socket.recv().await.is_some() {}
    })
}

/// A server that completes the WebSocket upgrade but never sends the expected
/// heartbeat must not be able to block `HeadlessHost::shutdown()` forever.
///
/// Without racing `do_handshake` against the shutdown signal, the receiver
/// runtime stays parked inside the handshake loop, the runtime task never
/// exits, and `shutdown()` hangs (the timeout below fires → test fails).
#[tokio::test]
async fn shutdown_completes_during_stalled_handshake() {
    let dir = tempfile::tempdir().expect("temp dir");

    // Start a server that upgrades the WS connection but never replies.
    let (upgraded_tx, upgraded_rx) = tokio::sync::oneshot::channel::<()>();
    let st = StallState {
        upgraded_tx: Arc::new(Mutex::new(Some(upgraded_tx))),
    };
    let app = Router::new()
        .route("/ws/v1.2/receivers", get(stalling_handler))
        .with_state(st);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stalling server");
    let server_addr = listener.local_addr().expect("server local_addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // Seed a profile so the headless runtime auto-connects to the stalling
    // server on startup. The DB connection is dropped before the host opens it.
    {
        let db_path = dir.path().join("receiver.sqlite3");
        let mut db = receiver::Db::open(&db_path).expect("open receiver DB for seeding");
        db.save_profile(
            &format!("ws://{server_addr}"),
            "test-token",
            "manual",
            Some("recv-test"),
        )
        .expect("seed profile");
    }

    let host = HeadlessHost::start(loopback_ephemeral_config(dir.path()))
        .await
        .expect("start headless host");

    // Wait until the server has received the receiver's hello, proving the
    // receiver is now parked inside the post-upgrade handshake awaiting a
    // heartbeat that will never arrive.
    tokio::time::timeout(Duration::from_secs(5), upgraded_rx)
        .await
        .expect("receiver should send hello to stalling server")
        .expect("handshake-parked signal");

    // Shutdown must complete promptly despite the stalled handshake.
    tokio::time::timeout(Duration::from_secs(5), host.shutdown())
        .await
        .expect("shutdown must not hang while handshake is stalled")
        .expect("shutdown result");
}
