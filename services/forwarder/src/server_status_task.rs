//! Background poll of the central server's status board.
//!
//! Refreshes the cached [`ServerDeviceStatus`] snapshot in the status store
//! every 30 seconds so the `/api/v1/status` HTTP handler can serve server
//! reachability without performing outbound I/O inline (local UI latency must
//! not depend on WAN state).

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::info;

use crate::config_service::ConfigState;
use crate::status_store::{ServerDeviceStatus, StatusStore};

/// How often the server status board is polled.
const POLL_INTERVAL: Duration = Duration::from_secs(30);

/// Timeout for each outbound status request.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Debug, serde::Deserialize)]
struct ServerStatusBoardJson {
    #[serde(default)]
    devices: Vec<ServerStatusDeviceJson>,
}

#[derive(Debug, serde::Deserialize)]
struct ServerStatusDeviceJson {
    endpoint_id: String,
    approval_state: String,
}

/// Spawn the background task that polls the server status board and stores
/// the resulting [`ServerDeviceStatus`] snapshot in `store`.
///
/// The first poll fires immediately; subsequent polls run every 30s. The task
/// exits when the shutdown watch channel changes.
pub fn spawn_server_status_task(
    config_state: Arc<ConfigState>,
    store: StatusStore,
    mut shutdown_rx: watch::Receiver<bool>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown_rx.changed() => {
                    info!("server status poll task shutting down");
                    return;
                }
            }
            let status = poll_server_status(&config_state, &store).await;
            store.set_server_status(status).await;
        }
    })
}

fn now_unix_ms() -> Option<i64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
}

fn checked(status: ServerDeviceStatus) -> ServerDeviceStatus {
    ServerDeviceStatus {
        checked_unix_ms: now_unix_ms(),
        ..status
    }
}

fn unreachable_status(endpoint_id: String, message: String) -> ServerDeviceStatus {
    checked(ServerDeviceStatus {
        configured: true,
        endpoint_id: Some(endpoint_id),
        reachable: Some(false),
        approval_state: None,
        waiting_for_approval: false,
        message: Some(message),
        cached: true,
        checked_unix_ms: None,
    })
}

/// Perform one poll of the server status board and produce the snapshot the
/// status endpoint will serve.
async fn poll_server_status(
    config_state: &Arc<ConfigState>,
    store: &StatusStore,
) -> ServerDeviceStatus {
    let endpoint_id = store.subsystem_arc().lock().await.p2p_endpoint_id.clone();
    let Some(endpoint_id) = endpoint_id else {
        return checked(ServerDeviceStatus::not_configured());
    };
    let server_url = {
        let _guard = config_state.write_lock.lock().await;
        match crate::config::load_config_from_path(&config_state.path) {
            Ok(config) => config.p2p.server_url,
            Err(error) => {
                return checked(ServerDeviceStatus {
                    configured: true,
                    endpoint_id: Some(endpoint_id),
                    reachable: None,
                    approval_state: None,
                    waiting_for_approval: false,
                    message: Some(format!("Forwarder config unavailable: {error}")),
                    cached: true,
                    checked_unix_ms: None,
                });
            }
        }
    };
    let Some(server_url) = server_url else {
        return checked(ServerDeviceStatus::not_configured());
    };

    let client = match reqwest::Client::builder().timeout(REQUEST_TIMEOUT).build() {
        Ok(client) => client,
        Err(error) => {
            return unreachable_status(
                endpoint_id,
                format!("Server status client unavailable: {error}"),
            );
        }
    };
    let status_url = format!("{}/status", server_url.trim_end_matches('/'));
    let response = match client.get(status_url).send().await {
        Ok(response) => response,
        Err(error) => {
            return unreachable_status(endpoint_id, format!("Server status unavailable: {error}"));
        }
    };
    let board = match response.error_for_status() {
        Ok(response) => match response.json::<ServerStatusBoardJson>().await {
            Ok(board) => board,
            Err(error) => {
                return unreachable_status(
                    endpoint_id,
                    format!("Server status response was invalid: {error}"),
                );
            }
        },
        Err(error) => {
            return unreachable_status(
                endpoint_id,
                format!("Server status returned an error: {error}"),
            );
        }
    };

    match board
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == endpoint_id)
    {
        Some(device) => {
            let waiting_for_approval = device.approval_state == "pending";
            checked(ServerDeviceStatus {
                configured: true,
                endpoint_id: Some(endpoint_id),
                reachable: Some(true),
                approval_state: Some(device.approval_state),
                waiting_for_approval,
                message: waiting_for_approval
                    .then(|| "Waiting for server admin approval".to_owned()),
                cached: true,
                checked_unix_ms: None,
            })
        }
        None => checked(ServerDeviceStatus {
            configured: true,
            endpoint_id: Some(endpoint_id),
            reachable: Some(true),
            approval_state: None,
            waiting_for_approval: true,
            message: Some("Waiting for this forwarder to register with the server".to_owned()),
            cached: true,
            checked_unix_ms: None,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status_store::SubsystemStatus;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn config_file_with_server_url(server_url: &str) -> (tempfile::TempDir, NamedTempFile) {
        let token_dir = tempfile::tempdir().expect("create temp token dir");
        let token_path = token_dir.path().join("fake-token");
        std::fs::write(&token_path, "test-token\n").expect("write token file");
        let token_path = token_path.display().to_string().replace('\\', "/");

        let mut config_file = NamedTempFile::new().expect("create temp config");
        write!(
            config_file,
            r#"schema_version = 1
[p2p]
server_url = "{server_url}"
[auth]
token_file = "{token_path}"
[[readers]]
target = "192.168.1.100:10000"
"#
        )
        .expect("write config");
        (token_dir, config_file)
    }

    fn store_with_endpoint_id(endpoint_id: &str) -> StatusStore {
        let mut subsystem = SubsystemStatus::ready();
        subsystem.set_p2p_endpoint_id(endpoint_id.to_owned());
        StatusStore::new(subsystem)
    }

    async fn serve_status_board_once(body: String) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let addr = listener.local_addr().expect("mock server addr");
        tokio::spawn(async move {
            if let Ok((mut socket, _)) = listener.accept().await {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buf = [0u8; 1024];
                let _ = socket.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn poll_reports_reachable_active_device() {
        let body = r#"{"devices":[{"endpoint_id":"ep-1","approval_state":"active"}]}"#;
        let addr = serve_status_board_once(body.to_owned()).await;
        let (_token_dir, config_file) = config_file_with_server_url(&format!("http://{addr}"));
        let config_state = Arc::new(ConfigState::new(config_file.path().to_path_buf()));
        let store = store_with_endpoint_id("ep-1");

        let status = poll_server_status(&config_state, &store).await;

        assert!(status.configured);
        assert_eq!(status.endpoint_id.as_deref(), Some("ep-1"));
        assert_eq!(status.reachable, Some(true));
        assert_eq!(status.approval_state.as_deref(), Some("active"));
        assert!(!status.waiting_for_approval);
        assert!(status.cached);
        assert!(status.checked_unix_ms.is_some());
    }

    #[tokio::test]
    async fn poll_reports_unreachable_when_server_never_responds() {
        // Accepts connections but never responds.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind hang listener");
        let addr = listener.local_addr().expect("hang listener addr");
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => held.push(s),
                    Err(_) => break,
                }
            }
        });
        let (_token_dir, config_file) = config_file_with_server_url(&format!("http://{addr}"));
        let config_state = Arc::new(ConfigState::new(config_file.path().to_path_buf()));
        let store = store_with_endpoint_id("ep-1");

        let status = poll_server_status(&config_state, &store).await;

        assert!(status.configured);
        assert_eq!(status.reachable, Some(false));
        assert!(status.cached);
        assert!(status.checked_unix_ms.is_some());
        assert!(status.message.is_some());
    }

    #[tokio::test]
    async fn poll_reports_not_configured_without_endpoint_id() {
        let (_token_dir, config_file) = config_file_with_server_url("http://127.0.0.1:9");
        let config_state = Arc::new(ConfigState::new(config_file.path().to_path_buf()));
        let store = StatusStore::new(SubsystemStatus::ready());

        let status = poll_server_status(&config_state, &store).await;

        assert!(!status.configured);
        assert_eq!(status.reachable, None);
        assert!(status.cached);
        assert!(status.checked_unix_ms.is_some());
    }

    #[tokio::test]
    async fn task_populates_store_and_stops_on_shutdown() {
        let body = r#"{"devices":[{"endpoint_id":"ep-1","approval_state":"pending"}]}"#;
        let addr = serve_status_board_once(body.to_owned()).await;
        let (_token_dir, config_file) = config_file_with_server_url(&format!("http://{addr}"));
        let config_state = Arc::new(ConfigState::new(config_file.path().to_path_buf()));
        let store = store_with_endpoint_id("ep-1");
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let handle = spawn_server_status_task(config_state, store.clone(), shutdown_rx);

        let mut snapshot = None;
        for _ in 0..100 {
            snapshot = store.server_status().await;
            if snapshot.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let snapshot = snapshot.expect("poll task populated server status");
        assert_eq!(snapshot.reachable, Some(true));
        assert_eq!(snapshot.approval_state.as_deref(), Some("pending"));
        assert!(snapshot.waiting_for_approval);
        assert!(snapshot.cached);
        assert!(snapshot.checked_unix_ms.is_some());

        shutdown_tx.send(true).expect("send shutdown");
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("task exits on shutdown")
            .expect("task join");
    }
}
