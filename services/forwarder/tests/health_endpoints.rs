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

use forwarder::status_http::{StatusServer, StatusConfig, SubsystemStatus};
use std::net::SocketAddr;
use std::time::Duration;

// Helper: make an HTTP request (using tokio's TcpStream for simplicity)
async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let mut stream = TcpStream::connect(addr).await.expect("connect failed");
    let request = format!("GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n", path);
    stream.write_all(request.as_bytes()).await.expect("write failed");

    let mut response = String::new();
    stream.read_to_string(&mut response).await.expect("read failed");

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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let mut stream = TcpStream::connect(addr).await.expect("connect failed");
    let request = format!(
        "POST {} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        path,
        body.len(),
        body
    );
    stream.write_all(request.as_bytes()).await.expect("write failed");

    let mut response = String::new();
    stream.read_to_string(&mut response).await.expect("read failed");

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
    let server = StatusServer::start(cfg, subsystem).await.expect("start failed");
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
    let server = StatusServer::start(cfg, subsystem).await.expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _body) = http_get(addr, "/readyz").await;
    assert_eq!(status, 200, "/readyz must return 200 when subsystems are ready");
}

#[tokio::test]
async fn readyz_returns_503_when_not_ready() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    // Subsystem is NOT ready
    let subsystem = SubsystemStatus::not_ready("journal not initialized".to_owned());
    let server = StatusServer::start(cfg, subsystem).await.expect("start failed");
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

    let server = StatusServer::start(cfg, subsystem).await.expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _body) = http_get(addr, "/readyz").await;
    assert_eq!(status, 200, "/readyz must return 200 regardless of uplink state");
}

#[tokio::test]
async fn epoch_reset_endpoint_returns_200() {
    use tempfile::tempdir;
    use forwarder::storage::journal::Journal;

    let dir = tempdir().expect("tempdir failed");
    let db_path = dir.path().join("test.sqlite3");
    let mut journal = Journal::open(&db_path).expect("journal open failed");
    journal.ensure_stream_state("192.168.1.5", 1).expect("ensure stream failed");

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
    let (status, _body) = http_post(addr, "/api/v1/streams/192.168.1.5/reset-epoch", "").await;
    assert_eq!(status, 200, "reset-epoch endpoint must return 200");

    // Verify epoch was bumped
    let mut j = shared_journal.lock().await;
    let (epoch, next_seq) = j.current_epoch_and_next_seq("192.168.1.5").expect("get epoch failed");
    assert_eq!(epoch, 2, "epoch must have been bumped to 2");
    assert_eq!(next_seq, 1, "next_seq must be reset to 1 after epoch bump");
}

#[tokio::test]
async fn epoch_reset_preserves_old_epoch_events() {
    use tempfile::tempdir;
    use forwarder::storage::journal::Journal;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    let dir = tempdir().expect("tempdir failed");
    let db_path = dir.path().join("test.sqlite3");
    let mut journal = Journal::open(&db_path).expect("journal open failed");
    journal.ensure_stream_state("192.168.1.10", 1).expect("ensure stream failed");

    // Insert some events in epoch 1 (unacked)
    journal.insert_event("192.168.1.10", 1, 1, None, "READ1", "raw").expect("insert failed");
    journal.insert_event("192.168.1.10", 1, 2, None, "READ2", "raw").expect("insert failed");

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
    let old_epoch_count = j.count_events_for_epoch("192.168.1.10", 1).expect("count failed");
    assert_eq!(old_epoch_count, 2, "old-epoch events must not be deleted by epoch reset");
}

#[tokio::test]
async fn epoch_reset_unknown_stream_returns_404() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();

    // Start without journal — just bare server
    let server = StatusServer::start(cfg, subsystem).await.expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(addr, "/api/v1/streams/1.2.3.4/reset-epoch", "").await;
    assert_eq!(status, 404, "unknown stream must return 404");
}

#[tokio::test]
async fn status_page_returns_html() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem).await.expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, body) = http_get(addr, "/").await;
    assert_eq!(status, 200, "status page must return 200");
    assert!(
        body.contains("text/html"),
        "response must be HTML content-type, got: {}",
        &body[..200.min(body.len())]
    );
    assert!(
        body.contains("0.1.0-test"),
        "status page must include forwarder version"
    );
}

#[tokio::test]
async fn unknown_path_returns_404() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let server = StatusServer::start(cfg, subsystem).await.expect("start failed");
    let addr = server.local_addr();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_get(addr, "/no/such/path").await;
    assert_eq!(status, 404, "unknown path must return 404");
}
