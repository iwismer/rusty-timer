//! Integration tests for Task 8: Status HTTP Service.
//!
//! Tests:
//! 1. /healthz returns 200
//! 2. /readyz returns 200 when local subsystems ready (not dependent on uplink)
//! 3. /readyz returns 503 when subsystems not initialized
//! 4. POST /api/v1/streams/{reader_ip}/reset-epoch triggers epoch bump
//! 5. epoch reset preserves old-epoch unacked events
//! 6. status page returns HTML with expected content
//! 7. graceful shutdown handler registered

use forwarder::status_http::{StatusConfig, StatusServer, SubsystemStatus};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

// Helper: make an HTTP request (using tokio's TcpStream for simplicity)
async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let mut stream = TcpStream::connect(addr).await.expect("connect failed");
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        path
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write failed");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read failed");

    // Parse status code from first line: "HTTP/1.1 200 OK"
    let status: u16 = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("could not parse status code");

    (status, response)
}

async fn http_post(addr: SocketAddr, path: &str, body: &str) -> (u16, String) {
    http_with_method(addr, "POST", path, body).await
}

async fn http_put(addr: SocketAddr, path: &str, body: &str) -> (u16, String) {
    http_with_method(addr, "PUT", path, body).await
}

async fn http_with_method(addr: SocketAddr, method: &str, path: &str, body: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let mut stream = TcpStream::connect(addr).await.expect("connect failed");
    let request = format!(
        "{} {} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        method,
        path,
        body.len(),
        body
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write failed");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read failed");

    let status: u16 = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .expect("could not parse status code");

    (status, response)
}

#[tokio::test]
async fn healthz_returns_200() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _body) = http_get(addr, "/healthz").await;
    assert_eq!(status, 200, "/healthz must return 200");
}

#[tokio::test]
async fn readyz_returns_200_when_ready() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    // Subsystem is ready: config loaded, journal open, workers started
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _body) = http_get(addr, "/readyz").await;
    assert_eq!(
        status, 200,
        "/readyz must return 200 when subsystems are ready"
    );
}

#[tokio::test]
async fn readyz_returns_503_when_not_ready() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    // Subsystem is NOT ready
    let subsystem = SubsystemStatus::not_ready("journal not initialized".to_owned());
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _body) = http_get(addr, "/readyz").await;
    assert_eq!(status, 503, "/readyz must return 503 when not ready");
}

#[tokio::test]
async fn readyz_independent_of_uplink() {
    // Key contract: /readyz should be ready even if uplink is NOT connected.
    // SubsystemStatus represents local readiness only (config + journal + loops).
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let mut subsystem = SubsystemStatus::ready();
    // Simulate uplink being disconnected — this must NOT affect readyz
    subsystem.set_uplink_connected(false);

    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _body) = http_get(addr, "/readyz").await;
    assert_eq!(
        status, 200,
        "/readyz must return 200 regardless of uplink state"
    );
}

#[tokio::test]
async fn epoch_reset_endpoint_returns_200() {
    use forwarder::storage::journal::Journal;
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir failed");
    let db_path = dir.path().join("test.sqlite3");
    let mut journal = Journal::open(&db_path).expect("journal open failed");
    journal
        .ensure_stream_state("192.168.1.5:10000", 1)
        .expect("ensure stream failed");

    // Wrap journal in Arc<Mutex> for shared access
    use std::sync::Arc;
    use tokio::sync::Mutex;
    let shared_journal = Arc::new(Mutex::new(journal));

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start_with_journal(cfg, subsystem, shared_journal.clone())
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // POST epoch reset
    let (status, _body) =
        http_post(addr, "/api/v1/streams/192.168.1.5:10000/reset-epoch", "").await;
    assert_eq!(status, 200, "reset-epoch endpoint must return 200");

    // Verify epoch was bumped
    let mut j = shared_journal.lock().await;
    let (epoch, next_seq) = j
        .current_epoch_and_next_seq("192.168.1.5:10000")
        .expect("get epoch failed");
    assert_eq!(epoch, 2, "epoch must have been bumped to 2");
    assert_eq!(next_seq, 1, "next_seq must be reset to 1 after epoch bump");
}

#[tokio::test]
async fn epoch_reset_endpoint_accepts_percent_encoded_stream_key() {
    use forwarder::storage::journal::Journal;
    use tempfile::tempdir;

    let dir = tempdir().expect("tempdir failed");
    let db_path = dir.path().join("test.sqlite3");
    let mut journal = Journal::open(&db_path).expect("journal open failed");
    journal
        .ensure_stream_state("192.168.1.6:10000", 1)
        .expect("ensure stream failed");

    use std::sync::Arc;
    use tokio::sync::Mutex;
    let shared_journal = Arc::new(Mutex::new(journal));

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start_with_journal(cfg, subsystem, shared_journal.clone())
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _body) =
        http_post(addr, "/api/v1/streams/192.168.1.6%3A10000/reset-epoch", "").await;
    assert_eq!(status, 200, "encoded reset path must return 200");

    let mut j = shared_journal.lock().await;
    let (epoch, _next_seq) = j
        .current_epoch_and_next_seq("192.168.1.6:10000")
        .expect("get epoch failed");
    assert_eq!(epoch, 2, "encoded path must bump matching stream");
}

#[tokio::test]
async fn epoch_reset_preserves_old_epoch_events() {
    use forwarder::storage::journal::Journal;
    use std::sync::Arc;
    use tempfile::tempdir;
    use tokio::sync::Mutex;

    let dir = tempdir().expect("tempdir failed");
    let db_path = dir.path().join("test.sqlite3");
    let mut journal = Journal::open(&db_path).expect("journal open failed");
    journal
        .ensure_stream_state("192.168.1.10", 1)
        .expect("ensure stream failed");

    // Insert some events in epoch 1 (unacked)
    journal
        .insert_event("192.168.1.10", 1, 1, None, b"READ1", "raw")
        .expect("insert failed");
    journal
        .insert_event("192.168.1.10", 1, 2, None, b"READ2", "raw")
        .expect("insert failed");

    let shared_journal = Arc::new(Mutex::new(journal));

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start_with_journal(cfg, subsystem, shared_journal.clone())
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Reset epoch
    let (status, _) = http_post(addr, "/api/v1/streams/192.168.1.10/reset-epoch", "").await;
    assert_eq!(status, 200);

    // Old-epoch events must still be present in the journal
    let j = shared_journal.lock().await;
    let old_epoch_count = j
        .count_events_for_epoch("192.168.1.10", 1)
        .expect("count failed");
    assert_eq!(
        old_epoch_count, 2,
        "old-epoch events must not be deleted by epoch reset"
    );
}

#[tokio::test]
async fn epoch_reset_unknown_stream_returns_404() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();

    // Start without journal — just bare server
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(addr, "/api/v1/streams/1.2.3.4/reset-epoch", "").await;
    assert_eq!(status, 404, "unknown stream must return 404");
}

#[tokio::test]
async fn set_current_epoch_name_proxies_to_server_with_stream_lookup() {
    use axum::extract::{Path, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, put};
    use axum::{Json, Router};
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    #[derive(Clone, Default)]
    struct UpstreamCapture {
        auth_header: Arc<Mutex<Option<String>>>,
        stream_id: Arc<Mutex<Option<String>>>,
        epoch: Arc<Mutex<Option<i64>>>,
        name: Arc<Mutex<Option<Value>>>,
    }

    async fn list_streams_handler() -> Json<Value> {
        Json(serde_json::json!({
            "streams": [
                {
                    "stream_id": "11111111-1111-1111-1111-111111111111",
                    "forwarder_id": "fwd-abc123",
                    "reader_ip": "192.168.1.5",
                    "stream_epoch": 7,
                    "online": true,
                }
            ]
        }))
    }

    async fn put_epoch_name_handler(
        State(capture): State<UpstreamCapture>,
        Path((stream_id, epoch)): Path<(String, i64)>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        let auth = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .map(ToOwned::to_owned);
        *capture.auth_header.lock().await = auth;
        *capture.stream_id.lock().await = Some(stream_id.clone());
        *capture.epoch.lock().await = Some(epoch);
        *capture.name.lock().await = body.get("name").cloned();
        (
            StatusCode::OK,
            Json(serde_json::json!({
                "stream_id": stream_id,
                "stream_epoch": epoch,
                "name": body.get("name").cloned(),
            })),
        )
    }

    let capture = UpstreamCapture::default();
    let app = Router::new()
        .route("/api/v1/streams", get(list_streams_handler))
        .route(
            "/api/v1/streams/{stream_id}/epochs/{epoch}/name",
            put(put_epoch_name_handler),
        )
        .with_state(capture.clone());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let upstream_addr = listener.local_addr().expect("upstream addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });

    let mut token_file = NamedTempFile::new().expect("create token file");
    writeln!(token_file, "test-token").expect("write token");

    let mut config_file = NamedTempFile::new().expect("create config file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "http://{}"
[auth]
token_file = "{}"
[[readers]]
target = "192.168.1.5:10000"
"#,
        upstream_addr,
        token_file.path().display()
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        Arc::new(tokio::sync::Mutex::new(NoJournalForNameApi)),
        Arc::new(forwarder::status_http::ConfigState::new(
            config_file.path().to_path_buf(),
        )),
        Arc::new(tokio::sync::Notify::new()),
    )
    .await
    .expect("start failed");
    server.set_forwarder_id("fwd-abc123").await;
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _body) = http_put(
        addr,
        "/api/v1/streams/192.168.1.5/current-epoch/name",
        r#"{"name":"Lap 2"}"#,
    )
    .await;
    assert_eq!(
        status, 200,
        "set current epoch name endpoint must return 200"
    );

    assert_eq!(
        *capture.auth_header.lock().await,
        Some("Bearer test-token".to_owned())
    );
    assert_eq!(
        *capture.stream_id.lock().await,
        Some("11111111-1111-1111-1111-111111111111".to_owned())
    );
    assert_eq!(*capture.epoch.lock().await, Some(7));
    assert_eq!(
        *capture.name.lock().await,
        Some(Value::String("Lap 2".to_owned()))
    );
}

#[tokio::test]
async fn clear_current_epoch_name_sends_null_name() {
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::routing::{get, put};
    use axum::{Json, Router};
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    #[derive(Clone, Default)]
    struct UpstreamCapture {
        name: Arc<Mutex<Option<Value>>>,
    }

    async fn list_streams_handler() -> Json<Value> {
        Json(serde_json::json!({
            "streams": [
                {
                    "stream_id": "22222222-2222-2222-2222-222222222222",
                    "forwarder_id": "fwd-clear",
                    "reader_ip": "10.0.0.8",
                    "stream_epoch": 3,
                    "online": true,
                }
            ]
        }))
    }

    async fn put_epoch_name_handler(
        State(capture): State<UpstreamCapture>,
        Path((_stream_id, _epoch)): Path<(String, i64)>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        *capture.name.lock().await = body.get("name").cloned();
        (StatusCode::OK, Json(serde_json::json!({"ok": true})))
    }

    let capture = UpstreamCapture::default();
    let app = Router::new()
        .route("/api/v1/streams", get(list_streams_handler))
        .route(
            "/api/v1/streams/{stream_id}/epochs/{epoch}/name",
            put(put_epoch_name_handler),
        )
        .with_state(capture.clone());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let upstream_addr = listener.local_addr().expect("upstream addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });

    let mut token_file = NamedTempFile::new().expect("create token file");
    writeln!(token_file, "clear-token").expect("write token");

    let mut config_file = NamedTempFile::new().expect("create config file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "http://{}"
[auth]
token_file = "{}"
[[readers]]
target = "10.0.0.8:10000"
"#,
        upstream_addr,
        token_file.path().display()
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        Arc::new(tokio::sync::Mutex::new(NoJournalForNameApi)),
        Arc::new(forwarder::status_http::ConfigState::new(
            config_file.path().to_path_buf(),
        )),
        Arc::new(tokio::sync::Notify::new()),
    )
    .await
    .expect("start failed");
    server.set_forwarder_id("fwd-clear").await;
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _body) = http_put(
        addr,
        "/api/v1/streams/10.0.0.8/current-epoch/name",
        r#"{"name":null}"#,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(*capture.name.lock().await, Some(Value::Null));
}

/// Validates that `%3A` (percent-encoded colon) in the reader IP path segment is decoded
/// correctly by Axum and the request reaches the upstream — not rejected with 400.
#[tokio::test]
async fn set_current_epoch_name_accepts_percent_encoded_reader_ip() {
    use axum::extract::{Path, State};
    use axum::http::StatusCode;
    use axum::routing::{get, put};
    use axum::{Json, Router};
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tokio::net::TcpListener;
    use tokio::sync::Mutex;

    #[derive(Clone, Default)]
    struct Capture {
        name: Arc<Mutex<Option<Value>>>,
    }

    async fn list_streams_pct() -> Json<Value> {
        Json(serde_json::json!({
            "streams": [{
                "stream_id": "33333333-3333-3333-3333-333333333333",
                "forwarder_id": "fwd-pct",
                "reader_ip": "192.168.1.7:10000",
                "stream_epoch": 1,
                "online": true,
            }]
        }))
    }

    async fn put_name_pct(
        State(cap): State<Capture>,
        Path((_id, _epoch)): Path<(String, i64)>,
        Json(body): Json<Value>,
    ) -> (StatusCode, Json<Value>) {
        *cap.name.lock().await = body.get("name").cloned();
        (StatusCode::OK, Json(serde_json::json!({"ok": true})))
    }

    let capture = Capture::default();
    let app = Router::new()
        .route("/api/v1/streams", get(list_streams_pct))
        .route(
            "/api/v1/streams/{id}/epochs/{epoch}/name",
            put(put_name_pct),
        )
        .with_state(capture.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let upstream_addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

    let mut token_file = NamedTempFile::new().expect("token file");
    write!(token_file, "pct-token").expect("write token");

    let mut config_file = NamedTempFile::new().expect("config file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "http://{}"
[auth]
token_file = "{}"
[[readers]]
target = "192.168.1.7:10000"
"#,
        upstream_addr,
        token_file.path().display()
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        Arc::new(tokio::sync::Mutex::new(NoJournalForNameApi)),
        Arc::new(forwarder::status_http::ConfigState::new(
            config_file.path().to_path_buf(),
        )),
        Arc::new(tokio::sync::Notify::new()),
    )
    .await
    .expect("start failed");
    server.set_forwarder_id("fwd-pct").await;
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // %3A is a percent-encoded colon; Axum decodes it to ":" before the handler runs.
    // The old code incorrectly rejected any reader_ip still containing '%' — this
    // verifies that the fix allows correctly-encoded IPs through to the upstream.
    let (status, _body) = http_put(
        addr,
        "/api/v1/streams/192.168.1.7%3A10000/current-epoch/name",
        r#"{"name":"Pct Test"}"#,
    )
    .await;
    assert_eq!(
        status, 200,
        "percent-encoded colon in reader IP must be accepted"
    );
    assert_eq!(
        *capture.name.lock().await,
        Some(Value::String("Pct Test".to_owned()))
    );
}

#[tokio::test]
async fn set_current_epoch_name_returns_400_when_name_field_missing() {
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut token_file = NamedTempFile::new().expect("token file");
    write!(token_file, "tok").expect("write token");

    let mut config_file = NamedTempFile::new().expect("config file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "http://127.0.0.1:1"
[auth]
token_file = "{}"
[[readers]]
target = "10.0.0.1:10000"
"#,
        token_file.path().display()
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        Arc::new(tokio::sync::Mutex::new(NoJournalForNameApi)),
        Arc::new(forwarder::status_http::ConfigState::new(
            config_file.path().to_path_buf(),
        )),
        Arc::new(tokio::sync::Notify::new()),
    )
    .await
    .expect("start failed");
    server.set_forwarder_id("fwd-missing-name").await;
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // name field is absent — handler must return 400 before reaching upstream.
    let (status, _body) = http_put(
        addr,
        "/api/v1/streams/10.0.0.1/current-epoch/name",
        r#"{"other_field": "value"}"#,
    )
    .await;
    assert_eq!(status, 400, "missing name field must return 400");
}

#[tokio::test]
async fn set_current_epoch_name_returns_400_when_name_is_wrong_type() {
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut token_file = NamedTempFile::new().expect("token file");
    write!(token_file, "tok").expect("write token");

    let mut config_file = NamedTempFile::new().expect("config file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "http://127.0.0.1:1"
[auth]
token_file = "{}"
[[readers]]
target = "10.0.0.1:10000"
"#,
        token_file.path().display()
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        Arc::new(tokio::sync::Mutex::new(NoJournalForNameApi)),
        Arc::new(forwarder::status_http::ConfigState::new(
            config_file.path().to_path_buf(),
        )),
        Arc::new(tokio::sync::Notify::new()),
    )
    .await
    .expect("start failed");
    server.set_forwarder_id("fwd-wrong-type").await;
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // name is a number, not a string or null — handler must return 400.
    let (status, _body) = http_put(
        addr,
        "/api/v1/streams/10.0.0.1/current-epoch/name",
        r#"{"name": 42}"#,
    )
    .await;
    assert_eq!(status, 400, "non-string name must return 400");
}

/// Verifies that when the upstream epoch-name update fails, the error response from the
/// forwarder uses JSON content-type (not text/plain).
#[tokio::test]
async fn set_current_epoch_name_error_response_has_json_content_type() {
    use axum::extract::Path;
    use axum::http::StatusCode;
    use axum::routing::{get, put};
    use axum::{Json, Router};
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tokio::net::TcpListener;

    async fn list_streams_ct() -> Json<Value> {
        Json(serde_json::json!({
            "streams": [{
                "stream_id": "44444444-4444-4444-4444-444444444444",
                "forwarder_id": "fwd-ct",
                "reader_ip": "10.0.0.9",
                "stream_epoch": 5,
                "online": true,
            }]
        }))
    }

    async fn put_name_error(Path((_id, _epoch)): Path<(String, i64)>) -> (StatusCode, String) {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            "upstream error".to_owned(),
        )
    }

    let app = Router::new()
        .route("/api/v1/streams", get(list_streams_ct))
        .route(
            "/api/v1/streams/{id}/epochs/{epoch}/name",
            put(put_name_error),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let upstream_addr = listener.local_addr().expect("addr");
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

    let mut token_file = NamedTempFile::new().expect("token file");
    write!(token_file, "ct-token").expect("write token");

    let mut config_file = NamedTempFile::new().expect("config file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "http://{}"
[auth]
token_file = "{}"
[[readers]]
target = "10.0.0.9:10000"
"#,
        upstream_addr,
        token_file.path().display()
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        Arc::new(tokio::sync::Mutex::new(NoJournalForNameApi)),
        Arc::new(forwarder::status_http::ConfigState::new(
            config_file.path().to_path_buf(),
        )),
        Arc::new(tokio::sync::Notify::new()),
    )
    .await
    .expect("start failed");
    server.set_forwarder_id("fwd-ct").await;
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_put(
        addr,
        "/api/v1/streams/10.0.0.9/current-epoch/name",
        r#"{"name":"test"}"#,
    )
    .await;
    assert_eq!(status, 500, "upstream error must propagate as 500");
    assert!(
        response
            .to_lowercase()
            .contains("content-type: application/json"),
        "error response must have JSON content-type, got: {response}",
    );
}

struct NoJournalForNameApi;

impl forwarder::status_http::JournalAccess for NoJournalForNameApi {
    fn reset_epoch(
        &mut self,
        _stream_key: &str,
    ) -> Result<i64, forwarder::status_http::EpochResetError> {
        Err(forwarder::status_http::EpochResetError::NotFound)
    }

    fn event_count(&self, _stream_key: &str) -> Result<i64, String> {
        Ok(0)
    }
}

#[tokio::test]
async fn status_json_returns_version() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, body) = http_get(addr, "/api/v1/status").await;
    assert_eq!(status, 200, "status JSON must return 200");
    assert!(
        body.contains("0.1.0-test"),
        "status JSON must include forwarder version"
    );
}

#[tokio::test]
async fn unknown_api_path_returns_404() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_get(addr, "/api/no/such/path").await;
    assert_eq!(status, 404, "unknown API path must return 404");
}

#[tokio::test]
async fn unknown_update_path_returns_404() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_get(addr, "/update/no/such/path").await;
    assert_eq!(status, 404, "unknown update path must return 404");
}

#[tokio::test]
async fn bare_update_path_returns_404() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_get(addr, "/update").await;
    assert_eq!(status, 404, "bare update path must return 404");
}

#[tokio::test]
async fn status_json_shows_forwarder_id() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    server.set_forwarder_id("fwd-abc123").await;
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, body) = http_get(addr, "/api/v1/status").await;
    assert_eq!(status, 200);
    assert!(
        body.contains("fwd-abc123"),
        "status JSON must show forwarder ID"
    );
}

#[tokio::test]
async fn status_json_shows_reader_status() {
    use forwarder::status_http::ReaderConnectionState;

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    server.init_readers(&[("10.0.0.1".to_owned(), 10001)]).await;
    server
        .update_reader_state("10.0.0.1", ReaderConnectionState::Connected)
        .await;
    server.record_read("10.0.0.1").await;
    server.record_read("10.0.0.1").await;
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, body) = http_get(addr, "/api/v1/status").await;
    assert_eq!(status, 200);
    assert!(body.contains("10.0.0.1"), "status JSON must show reader IP");
    assert!(
        body.contains("connected"),
        "status JSON must show connection state"
    );
    assert!(
        body.contains("\"local_port\":10001"),
        "status JSON must show local port"
    );
}

#[tokio::test]
async fn record_read_increments_counter() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem)
        .await
        .expect("start failed");
    server.init_readers(&[("10.0.0.5".to_owned(), 10005)]).await;
    for _ in 0..5 {
        server.record_read("10.0.0.5").await;
    }
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, body) = http_get(addr, "/api/v1/status").await;
    assert_eq!(status, 200);
    // The JSON response should show reads_session of 5
    assert!(
        body.contains("\"reads_session\":5"),
        "status JSON must show session read count of 5"
    );
}

#[tokio::test]
async fn status_page_does_not_query_journal_for_totals() {
    use forwarder::status_http::{EpochResetError, JournalAccess};
    use tokio::sync::Mutex;

    struct CountingJournal {
        event_count_calls: Arc<AtomicUsize>,
    }

    impl JournalAccess for CountingJournal {
        fn reset_epoch(&mut self, _stream_key: &str) -> Result<i64, EpochResetError> {
            Ok(1)
        }

        fn event_count(&self, _stream_key: &str) -> Result<i64, String> {
            self.event_count_calls.fetch_add(1, Ordering::Relaxed);
            Ok(42)
        }
    }

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let calls = Arc::new(AtomicUsize::new(0));
    let journal = Arc::new(Mutex::new(CountingJournal {
        event_count_calls: calls.clone(),
    }));

    let server = StatusServer::start_with_journal(cfg, subsystem, journal)
        .await
        .expect("start failed");
    server.init_readers(&[("10.0.0.9".to_owned(), 10009)]).await;
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_get(addr, "/").await;
    assert_eq!(status, 200);

    assert_eq!(
        calls.load(Ordering::Relaxed),
        0,
        "status page must not query journal totals during rendering"
    );
}
