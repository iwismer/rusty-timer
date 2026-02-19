//! Integration tests for forwarder config editing endpoints.

use forwarder::status_http::{EpochResetError, JournalAccess};
use forwarder::status_http::{StatusConfig, StatusServer, SubsystemStatus};
use std::net::SocketAddr;
use std::time::Duration;

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
        "POST {} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
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

/// Extract the HTTP response body (everything after the blank line separating headers from body).
fn response_body(response: &str) -> &str {
    response
        .find("\r\n\r\n")
        .map(|i| &response[i + 4..])
        .unwrap_or("")
}

struct NoopJournal;

impl JournalAccess for NoopJournal {
    fn reset_epoch(&mut self, _stream_key: &str) -> Result<i64, EpochResetError> {
        Err(EpochResetError::NotFound)
    }
    fn event_count(&self, _stream_key: &str) -> Result<i64, String> {
        Ok(0)
    }
}

#[tokio::test]
async fn restart_needed_flag_defaults_to_false() {
    let ss = SubsystemStatus::ready();
    assert!(!ss.restart_needed(), "restart_needed must default to false");
}

#[tokio::test]
async fn get_config_returns_json() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
display_name = "Start Line"

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "/tmp/fake-token"

[[readers]]
target = "192.168.1.100:10000"
"#
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let config_state = ConfigState::new(config_file.path().to_path_buf());

    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let server = StatusServer::start_with_config(cfg, subsystem, journal, config_state)
        .await
        .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_get(addr, "/api/v1/config").await;
    assert_eq!(status, 200);

    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");
    assert_eq!(json["display_name"], "Start Line");
    assert_eq!(json["server"]["base_url"], "https://timing.example.com");
}
