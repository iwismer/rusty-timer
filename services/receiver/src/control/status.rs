//! Status, metrics, and log control handlers.

use crate::control_api::{AppState, ConnectionState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServerDeviceStatus {
    pub configured: bool,
    pub endpoint_id: Option<String>,
    pub reachable: Option<bool>,
    pub approval_state: Option<String>,
    pub waiting_for_approval: bool,
    pub message: Option<String>,
}

impl ServerDeviceStatus {
    fn not_configured() -> Self {
        Self {
            configured: false,
            endpoint_id: None,
            reachable: None,
            approval_state: None,
            waiting_for_approval: false,
            message: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub receiver_id: String,
    pub connection_state: ConnectionState,
    pub local_ok: bool,
    pub streams_count: usize,
    pub server: ServerDeviceStatus,
}

#[derive(Debug, Serialize)]
pub struct LogsResponse {
    pub entries: Vec<String>,
}

pub async fn get_stream_metrics(state: &AppState) -> Vec<crate::ui_events::StreamMetricsPayload> {
    state.get_stream_metrics_snapshot().await
}

pub async fn get_status(state: &AppState) -> StatusResponse {
    let receiver_id = state.receiver_id.read().await.clone();
    let conn = state.connection_state.borrow().clone();
    let db = state.db.lock().await;
    let streams_count = db.load_stream_subscriptions().map(|s| s.len()).unwrap_or(0);
    let local_ok = state.db_integrity_ok;
    drop(db);
    let server = server_device_status(state).await;
    StatusResponse {
        receiver_id,
        connection_state: conn,
        local_ok,
        streams_count,
        server,
    }
}

#[derive(Debug, Deserialize)]
struct ServerStatusBoard {
    #[serde(default)]
    devices: Vec<ServerStatusDevice>,
}

#[derive(Debug, Deserialize)]
struct ServerStatusDevice {
    endpoint_id: String,
    approval_state: String,
}

pub(crate) async fn server_device_status(state: &AppState) -> ServerDeviceStatus {
    let server_url = {
        let profile = {
            let db = state.db.lock().await;
            db.load_profile().ok().flatten()
        };
        // Mirror the P2P runtime's resolution (env/CLI override > profile) so
        // the status card reflects the server the receiver actually targets.
        match crate::runtime::resolve_server_config(profile.as_ref(), state.server_override().await)
        {
            Some(server) => server.url,
            None => return ServerDeviceStatus::not_configured(),
        }
    };

    server_device_status_for_url(state, &server_url).await
}

pub(crate) async fn server_device_status_for_url(
    state: &AppState,
    server_url: &str,
) -> ServerDeviceStatus {
    let endpoint_id = state.p2p_endpoint_id.read().await.clone();

    let Some(endpoint_id) = endpoint_id else {
        return ServerDeviceStatus {
            configured: true,
            endpoint_id: None,
            reachable: None,
            approval_state: None,
            waiting_for_approval: true,
            message: Some("Waiting for the local P2P endpoint to start".to_owned()),
        };
    };

    let status_url = format!("{}/status", server_url.trim_end_matches('/'));
    let response = match state.http_client.get(status_url).send().await {
        Ok(response) => response,
        Err(error) => {
            return ServerDeviceStatus {
                configured: true,
                endpoint_id: Some(endpoint_id),
                reachable: Some(false),
                approval_state: None,
                waiting_for_approval: false,
                message: Some(format!("Server status unavailable: {error}")),
            };
        }
    };
    let board = match response.error_for_status() {
        Ok(response) => match response.json::<ServerStatusBoard>().await {
            Ok(board) => board,
            Err(error) => {
                return ServerDeviceStatus {
                    configured: true,
                    endpoint_id: Some(endpoint_id),
                    reachable: Some(false),
                    approval_state: None,
                    waiting_for_approval: false,
                    message: Some(format!("Server status response was invalid: {error}")),
                };
            }
        },
        Err(error) => {
            return ServerDeviceStatus {
                configured: true,
                endpoint_id: Some(endpoint_id),
                reachable: Some(false),
                approval_state: None,
                waiting_for_approval: false,
                message: Some(format!("Server status returned an error: {error}")),
            };
        }
    };

    match board
        .devices
        .into_iter()
        .find(|device| device.endpoint_id == endpoint_id)
    {
        Some(device) => {
            let waiting_for_approval = device.approval_state == "pending";
            ServerDeviceStatus {
                configured: true,
                endpoint_id: Some(endpoint_id),
                reachable: Some(true),
                approval_state: Some(device.approval_state),
                waiting_for_approval,
                message: waiting_for_approval
                    .then(|| "Waiting for server admin approval".to_owned()),
            }
        }
        None => ServerDeviceStatus {
            configured: true,
            endpoint_id: Some(endpoint_id),
            reachable: Some(true),
            approval_state: None,
            waiting_for_approval: true,
            message: Some("Waiting for this receiver to register with the server".to_owned()),
        },
    }
}

pub async fn get_logs(state: &AppState) -> LogsResponse {
    let entries = state.logger.entries();
    LogsResponse { entries }
}

pub fn get_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}
