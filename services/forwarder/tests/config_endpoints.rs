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

async fn http_post_split(addr: SocketAddr, path: &str, body: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let mut stream = TcpStream::connect(addr).await.expect("connect failed");
    let headers = format!(
        "POST {} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        path,
        body.len()
    );
    stream
        .write_all(headers.as_bytes())
        .await
        .expect("write headers failed");
    tokio::time::sleep(Duration::from_millis(25)).await;
    stream
        .write_all(body.as_bytes())
        .await
        .expect("write body failed");

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
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        subsystem,
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
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

#[tokio::test]
async fn post_config_general_updates_display_name() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1

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
    let config_path = config_file.path().to_path_buf();
    let config_state = ConfigState::new(config_path.clone());

    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        subsystem,
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // POST new display_name
    let (status, response) = http_post(
        addr,
        "/api/v1/config/general",
        r#"{"display_name":"Finish Line"}"#,
    )
    .await;
    assert_eq!(status, 200);
    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");
    assert_eq!(json["ok"], true);

    // Verify the TOML file was updated
    let toml_str = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        toml_str.contains("Finish Line"),
        "TOML file must contain updated display_name, got: {}",
        toml_str
    );

    // Verify restart_needed is set
    assert!(
        server.restart_needed().await,
        "restart_needed must be true after config change"
    );
}

#[tokio::test]
async fn post_config_general_updates_readonly_config_via_atomic_replace() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "/tmp/fake-token"

[[readers]]
target = "192.168.1.100:10000"
"#
    )
    .expect("write config");

    let config_path = config_file.path().to_path_buf();
    let mut perms = std::fs::metadata(&config_path)
        .expect("metadata")
        .permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&config_path, perms).expect("set readonly");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let subsystem = SubsystemStatus::ready();
    let config_state = ConfigState::new(config_path.clone());

    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        subsystem,
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/general",
        r#"{"display_name":"Atomic Replace"}"#,
    )
    .await;
    assert_eq!(
        status, 200,
        "readonly config should still be replaceable, response: {}",
        response
    );

    let toml_str = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        toml_str.contains("Atomic Replace"),
        "TOML file must contain updated display_name, got: {}",
        toml_str
    );
}

#[cfg(unix)]
#[tokio::test]
async fn post_config_general_preserves_file_mode_on_atomic_replace() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "/tmp/fake-token"

[[readers]]
target = "192.168.1.100:10000"
"#
    )
    .expect("write config");

    let config_path = config_file.path().to_path_buf();
    let expected_mode: u32 = 0o640;
    std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(expected_mode))
        .expect("set file mode");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let config_state = ConfigState::new(config_path.clone());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/general",
        r#"{"display_name":"Mode Preserve"}"#,
    )
    .await;
    assert_eq!(status, 200, "config update should succeed: {}", response);

    let actual_mode = std::fs::metadata(&config_path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        actual_mode, expected_mode,
        "atomic write must preserve file mode"
    );
}

#[tokio::test]
async fn post_config_general_accepts_fragmented_http_body() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1

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
    let config_path = config_file.path().to_path_buf();
    let config_state = ConfigState::new(config_path.clone());

    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        subsystem,
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post_split(
        addr,
        "/api/v1/config/general",
        r#"{"display_name":"Split Body"}"#,
    )
    .await;
    assert_eq!(
        status, 200,
        "fragmented body request should succeed, response: {}",
        response
    );

    let toml_str = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        toml_str.contains("Split Body"),
        "TOML file must contain updated display_name, got: {}",
        toml_str
    );
}

#[tokio::test]
async fn post_config_optional_sections_reject_non_object_payloads() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let endpoints = [
        "/api/v1/config/general",
        "/api/v1/config/journal",
        "/api/v1/config/uplink",
        "/api/v1/config/status_http",
        "/api/v1/config/control",
    ];
    for endpoint in endpoints {
        let (status, response) = http_post(addr, endpoint, r#""oops""#).await;
        assert_eq!(status, 400, "{} must reject scalar payloads", endpoint);
        let body = response_body(&response);
        assert!(
            body.contains("payload must be a JSON object"),
            "{} must report object payload requirement, body: {}",
            endpoint,
            body
        );
    }
}

#[tokio::test]
async fn post_config_server_updates_base_url() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "https://old.example.com"
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
    let config_path = config_file.path().to_path_buf();
    let config_state = ConfigState::new(config_path.clone());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/server",
        r#"{"base_url":"https://new.example.com","forwarders_ws_path":"/ws/v2/forwarders"}"#,
    )
    .await;
    assert_eq!(status, 200);
    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");
    assert_eq!(json["ok"], true);

    let toml_str = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        toml_str.contains("new.example.com"),
        "server base_url must be updated, got: {}",
        toml_str
    );
}

#[tokio::test]
async fn post_config_server_requires_base_url() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "https://old.example.com"
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(
        addr,
        "/api/v1/config/server",
        r#"{"forwarders_ws_path":"/ws/v2"}"#,
    )
    .await;
    assert_eq!(status, 400, "missing base_url must return 400");
}

#[tokio::test]
async fn post_config_server_rejects_empty_base_url() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "https://old.example.com"
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(addr, "/api/v1/config/server", r#"{"base_url":""}"#).await;
    assert_eq!(status, 400, "empty base_url must return 400");
}

#[tokio::test]
async fn post_config_server_rejects_invalid_base_url() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "https://old.example.com"
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(addr, "/api/v1/config/server", r#"{"base_url":"not-a-url"}"#).await;
    assert_eq!(status, 400, "invalid base_url must return 400");
}

#[tokio::test]
async fn post_config_auth_updates_token_file() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/old-token"
[[readers]]
target = "192.168.1.100:10000"
"#
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let config_path = config_file.path().to_path_buf();
    let config_state = ConfigState::new(config_path.clone());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/auth",
        r#"{"token_file":"/tmp/new-token"}"#,
    )
    .await;
    assert_eq!(status, 200);
    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");
    assert_eq!(json["ok"], true);

    let toml_str = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        toml_str.contains("new-token"),
        "auth token_file must be updated, got: {}",
        toml_str
    );
}

#[tokio::test]
async fn post_config_auth_requires_token_file() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(addr, "/api/v1/config/auth", r#"{}"#).await;
    assert_eq!(status, 400, "missing token_file must return 400");
}

#[tokio::test]
async fn post_config_auth_rejects_whitespace_token_file() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(addr, "/api/v1/config/auth", r#"{"token_file":"   "}"#).await;
    assert_eq!(status, 400, "whitespace token_file must return 400");
}

#[tokio::test]
async fn post_config_journal_updates_sqlite_path() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_path = config_file.path().to_path_buf();
    let config_state = ConfigState::new(config_path.clone());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/journal",
        r#"{"sqlite_path":"/data/journal.db","prune_watermark_pct":80}"#,
    )
    .await;
    assert_eq!(status, 200);
    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");
    assert_eq!(json["ok"], true);

    let toml_str = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        toml_str.contains("journal.db"),
        "journal sqlite_path must be updated, got: {}",
        toml_str
    );
    assert!(
        toml_str.contains("80"),
        "journal prune_watermark_pct must be updated, got: {}",
        toml_str
    );
}

#[tokio::test]
async fn post_config_journal_rejects_out_of_range_prune_watermark() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(
        addr,
        "/api/v1/config/journal",
        r#"{"prune_watermark_pct":1000}"#,
    )
    .await;
    assert_eq!(
        status, 400,
        "out-of-range prune_watermark_pct must return 400"
    );
}

#[tokio::test]
async fn post_config_journal_rejects_non_numeric_prune_watermark() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(
        addr,
        "/api/v1/config/journal",
        r#"{"prune_watermark_pct":"80"}"#,
    )
    .await;
    assert_eq!(
        status, 400,
        "non-numeric prune_watermark_pct must return 400"
    );
}

#[tokio::test]
async fn post_config_uplink_updates_batch_settings() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_path = config_file.path().to_path_buf();
    let config_state = ConfigState::new(config_path.clone());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/uplink",
        r#"{"batch_flush_ms":200,"batch_max_events":100}"#,
    )
    .await;
    assert_eq!(status, 200);
    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");
    assert_eq!(json["ok"], true);

    let toml_str = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        toml_str.contains("200"),
        "batch_flush_ms must be updated, got: {}",
        toml_str
    );
}

#[tokio::test]
async fn post_config_uplink_rejects_out_of_range_batch_max_events() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(
        addr,
        "/api/v1/config/uplink",
        r#"{"batch_max_events":5000000000}"#,
    )
    .await;
    assert_eq!(status, 400, "out-of-range batch_max_events must return 400");
}

#[tokio::test]
async fn post_config_status_http_updates_bind() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_path = config_file.path().to_path_buf();
    let config_state = ConfigState::new(config_path.clone());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/status_http",
        r#"{"bind":"0.0.0.0:9090"}"#,
    )
    .await;
    assert_eq!(status, 200);
    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");
    assert_eq!(json["ok"], true);

    let toml_str = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        toml_str.contains("0.0.0.0:9090"),
        "status_http bind must be updated, got: {}",
        toml_str
    );
}

#[tokio::test]
async fn post_config_status_http_rejects_invalid_ipv4_and_port() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(
        addr,
        "/api/v1/config/status_http",
        r#"{"bind":"999.999.999.999:99999"}"#,
    )
    .await;
    assert_eq!(status, 400, "invalid bind must return 400");
}

#[tokio::test]
async fn post_config_status_http_rejects_hostname_bind() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(
        addr,
        "/api/v1/config/status_http",
        r#"{"bind":"localhost:8080"}"#,
    )
    .await;
    assert_eq!(status, 400, "hostname bind must return 400");
}

#[tokio::test]
async fn post_config_status_http_rejects_ipv6_bind() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(
        addr,
        "/api/v1/config/status_http",
        r#"{"bind":"[::1]:8080"}"#,
    )
    .await;
    assert_eq!(status, 400, "ipv6 bind must return 400");
}

#[tokio::test]
async fn post_config_readers_replaces_list() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_path = config_file.path().to_path_buf();
    let config_state = ConfigState::new(config_path.clone());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/readers",
        r#"{"readers":[{"target":"192.168.1.200:10000","read_type":"raw","enabled":true},{"target":"192.168.1.201:10000"}]}"#,
    )
    .await;
    assert_eq!(status, 200);
    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");
    assert_eq!(json["ok"], true);

    let toml_str = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        toml_str.contains("192.168.1.200"),
        "new reader must be in TOML"
    );
    assert!(
        toml_str.contains("192.168.1.201"),
        "second reader must be in TOML"
    );
    assert!(
        !toml_str.contains("192.168.1.100"),
        "old reader must be removed from TOML"
    );
    assert!(
        !toml_str.contains("read_type"),
        "read_type must not be persisted in TOML"
    );
}

#[tokio::test]
async fn post_config_readers_validates_target() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Invalid target (CIDR notation, not supported)
    let (status, _) = http_post(
        addr,
        "/api/v1/config/readers",
        r#"{"readers":[{"target":"192.168.1.0/24:10000"}]}"#,
    )
    .await;
    assert_eq!(status, 400, "invalid target must return 400");
}

#[tokio::test]
async fn post_config_readers_rejects_out_of_range_local_fallback_port() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(
        addr,
        "/api/v1/config/readers",
        r#"{"readers":[{"target":"192.168.1.200:10000","local_fallback_port":70000}]}"#,
    )
    .await;
    assert_eq!(
        status, 400,
        "out-of-range local_fallback_port must return 400"
    );
}

#[tokio::test]
async fn post_config_readers_requires_at_least_one() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(addr, "/api/v1/config/readers", r#"{"readers":[]}"#).await;
    assert_eq!(status, 400, "empty readers list must return 400");
}

#[tokio::test]
async fn post_config_control_updates_allow_power_actions() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_path = config_file.path().to_path_buf();
    let config_state = ConfigState::new(config_path.clone());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/control",
        r#"{"allow_power_actions":true}"#,
    )
    .await;
    assert_eq!(status, 200);
    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");
    assert_eq!(json["ok"], true);

    let toml_str = std::fs::read_to_string(&config_path).expect("read config");
    assert!(
        toml_str.contains("[control]"),
        "config must include [control] section after update, got: {}",
        toml_str
    );
    assert!(
        toml_str.contains("allow_power_actions = true"),
        "control section must persist allow_power_actions=true, got: {}",
        toml_str
    );
    assert!(
        server.restart_needed().await,
        "restart_needed must be true after control config change"
    );
}

#[tokio::test]
async fn post_config_control_rejects_non_boolean_allow_power_actions() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/control",
        r#"{"allow_power_actions":"yes"}"#,
    )
    .await;
    assert_eq!(status, 400);
    let body = response_body(&response);
    assert!(
        body.contains("allow_power_actions must be a boolean or null"),
        "response must explain allow_power_actions type requirement, got: {}",
        body
    );
}

#[tokio::test]
async fn post_config_control_action_restart_device_requires_allow_power_actions_true() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/fake-token"
[control]
allow_power_actions = false
[[readers]]
target = "192.168.1.100:10000"
"#
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(
        addr,
        "/api/v1/config/control",
        r#"{"action":"restart_device"}"#,
    )
    .await;
    assert_eq!(status, 403);
    let body = response_body(&response);
    assert!(
        body.contains("power actions disabled"),
        "response must explain gating, got: {}",
        body
    );
}

#[tokio::test]
async fn restart_endpoint_returns_ok() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal.clone(),
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(addr, "/api/v1/restart", "{}").await;
    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");

    #[cfg(unix)]
    {
        assert_eq!(status, 200);
        assert_eq!(json["ok"], true);
        tokio::time::timeout(Duration::from_millis(200), restart_signal.notified())
            .await
            .expect("restart endpoint must notify restart signal");
    }

    #[cfg(not(unix))]
    {
        assert_eq!(status, 501);
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"], "restart not supported on non-unix platforms");
    }
}

#[tokio::test]
async fn restart_endpoint_returns_404_without_config() {
    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let server = StatusServer::start(cfg, SubsystemStatus::ready())
        .await
        .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(addr, "/api/v1/restart", "{}").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn control_restart_service_endpoint_returns_ok() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
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
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal.clone(),
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(addr, "/api/v1/control/restart-service", "{}").await;
    let body = response_body(&response);
    let json: serde_json::Value = serde_json::from_str(body).expect("parse JSON");

    #[cfg(unix)]
    {
        assert_eq!(status, 200);
        assert_eq!(json["ok"], true);
        tokio::time::timeout(Duration::from_millis(200), restart_signal.notified())
            .await
            .expect("restart service endpoint must notify restart signal");
    }

    #[cfg(not(unix))]
    {
        assert_eq!(status, 501);
        assert_eq!(json["ok"], false);
        assert_eq!(json["error"], "restart not supported on non-unix platforms");
    }
}

#[tokio::test]
async fn control_restart_device_requires_allow_power_actions_true() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/fake-token"
[control]
allow_power_actions = false
[[readers]]
target = "192.168.1.100:10000"
"#
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(addr, "/api/v1/control/restart-device", "{}").await;
    assert_eq!(status, 403);
    let body = response_body(&response);
    assert!(
        body.contains("power actions disabled"),
        "response must explain gating, got: {}",
        body
    );
}

#[tokio::test]
async fn control_shutdown_device_requires_allow_power_actions_true() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/fake-token"
[control]
allow_power_actions = false
[[readers]]
target = "192.168.1.100:10000"
"#
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, response) = http_post(addr, "/api/v1/control/shutdown-device", "{}").await;
    assert_eq!(status, 403);
    let body = response_body(&response);
    assert!(
        body.contains("power actions disabled"),
        "response must explain gating, got: {}",
        body
    );
}

#[tokio::test]
async fn control_action_errors_are_written_to_ui_logs() {
    use forwarder::status_http::ConfigState;
    use std::io::Write;
    use tempfile::NamedTempFile;

    let mut config_file = NamedTempFile::new().expect("create temp file");
    write!(
        config_file,
        r#"schema_version = 1
[server]
base_url = "https://timing.example.com"
[auth]
token_file = "/tmp/fake-token"
[control]
allow_power_actions = false
[[readers]]
target = "192.168.1.100:10000"
"#
    )
    .expect("write config");

    let cfg = StatusConfig {
        bind: "127.0.0.1:0".to_owned(),
        forwarder_version: "0.1.0-test".to_owned(),
    };
    let restart_signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let config_state = ConfigState::new(config_file.path().to_path_buf());
    let journal = std::sync::Arc::new(tokio::sync::Mutex::new(NoopJournal));
    let server = StatusServer::start_with_config(
        cfg,
        SubsystemStatus::ready(),
        journal,
        std::sync::Arc::new(config_state),
        restart_signal,
    )
    .await
    .expect("start failed");
    let addr = server.local_addr();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let (status, _) = http_post(addr, "/api/v1/control/shutdown-device", "{}").await;
    assert_eq!(status, 403);

    let (logs_status, logs_response) = http_get(addr, "/api/v1/logs").await;
    assert_eq!(logs_status, 200);
    let logs_json: serde_json::Value =
        serde_json::from_str(response_body(&logs_response)).expect("parse logs JSON");
    let entries = logs_json["entries"]
        .as_array()
        .expect("entries must be an array");
    let has_control_error = entries.iter().any(|entry| {
        entry
            .as_str()
            .is_some_and(|line| line.contains("control action 'shutdown_device' failed"))
    });
    assert!(
        has_control_error,
        "control failure should be present in UI logs, got: {}",
        logs_json
    );
}
