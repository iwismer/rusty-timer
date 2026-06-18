use receiver::headless::{HeadlessConfig, HeadlessHost};
use std::time::Duration;

fn loopback_ephemeral_config(data_dir: &std::path::Path) -> HeadlessConfig {
    HeadlessConfig {
        data_dir: data_dir.to_path_buf(),
        bind_addr: "127.0.0.1:0".parse().expect("parse bind addr"),
        receiver_id: None,
        p2p: None,
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
        p2p: None,
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
async fn p2p_startup_failure_does_not_leak_control_server() {
    use receiver::p2p_runtime::{ForwarderPeerConfig, P2pReceiverConfig};

    let dir = tempfile::tempdir().expect("temp dir");

    let bind_addr = {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("reserve port");
        let addr = listener.local_addr().expect("local addr");
        drop(listener);
        addr
    };

    let config = HeadlessConfig {
        data_dir: dir.path().to_path_buf(),
        bind_addr,
        receiver_id: None,
        p2p: Some(P2pReceiverConfig {
            secret_key_seed: [9u8; 32],
            forwarder: ForwarderPeerConfig {
                node_id: "not-a-valid-node-id".to_owned(),
                direct_addr: "127.0.0.1:5000".parse().expect("parse addr"),
            },
            thin_node: None,
            reconcile_interval: Duration::from_millis(100),
        }),
    };

    let err = match HeadlessHost::start(config).await {
        Ok(_) => panic!("invalid forwarder node id must fail P2P startup"),
        Err(err) => err,
    };
    assert!(
        err.contains("forwarder node id"),
        "error should describe the invalid forwarder node id, got: {err}"
    );

    let rebound = tokio::net::TcpListener::bind(bind_addr)
        .await
        .expect("control API port must be released after a failed P2P start");
    drop(rebound);
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
