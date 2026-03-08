use super::response::{gateway_timeout, json_error, not_found};
use crate::state::{AppState, ForwarderCommand, ForwarderProxyReply};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use rt_protocol::{ReaderControlAction, ReaderControlResponse};
use serde::Deserialize;
use std::time::Duration;

fn validate_reader_ip(reader_ip: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if reader_ip.contains(':') && reader_ip.split(':').all(|part| !part.is_empty()) {
        Ok(())
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "code": "INVALID_READER_IP",
                "message": format!("invalid reader_ip format: expected 'ip:port', got '{}'", reader_ip),
            })),
        ))
    }
}

/// Timeout for reader control request/response round trips.
/// Longer-running operations (SyncClock ~3s, ClearRecords ~10s) are handled
/// via fire-and-forget or background tasks on the forwarder side, so this
/// timeout only covers short command-response cycles.
const READER_CONTROL_TIMEOUT: Duration = Duration::from_secs(10);

/// Proxy an HTTP reader-control request to a connected forwarder via its
/// command channel. Looks up the forwarder's mpsc sender, sends the command,
/// then awaits a oneshot reply. The read lock on `forwarder_command_senders`
/// is dropped before the send to avoid holding it across the await.
///
/// Returns the forwarder's `ReaderControlResponse`, or an HTTP error if the
/// forwarder is not connected, the queue is saturated, or the response times out.
async fn send_reader_control(
    state: &AppState,
    forwarder_id: &str,
    reader_ip: &str,
    action: ReaderControlAction,
) -> Result<ReaderControlResponse, axum::response::Response> {
    validate_reader_ip(reader_ip).map_err(|e| e.into_response())?;
    tracing::debug!(forwarder_id = %forwarder_id, reader_ip = %reader_ip, action = ?action, "sending reader control request");
    let senders = state.forwarder_command_senders.read().await;
    let tx = match senders.get(forwarder_id) {
        Some(tx) => tx.clone(),
        None => return Err(not_found("forwarder not connected")),
    };
    drop(senders);

    let request_id = uuid::Uuid::new_v4().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ForwarderCommand::ReaderControl {
        request_id,
        reader_ip: reader_ip.to_owned(),
        action,
        reply: reply_tx,
    };

    match tokio::time::timeout(READER_CONTROL_TIMEOUT, tx.send(cmd)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            return Err(not_found("forwarder disconnected"));
        }
        Err(_) => {
            return Err(gateway_timeout("forwarder command queue is saturated"));
        }
    }

    match tokio::time::timeout(READER_CONTROL_TIMEOUT, reply_rx).await {
        Ok(Ok(ForwarderProxyReply::Response(resp))) => Ok(resp),
        Ok(Ok(ForwarderProxyReply::Timeout)) => {
            Err(gateway_timeout("forwarder did not respond within timeout"))
        }
        Ok(Ok(ForwarderProxyReply::InternalError(msg))) => {
            tracing::error!(forwarder_id = %forwarder_id, error = %msg, "internal error in reader control proxy");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "code": "INTERNAL_ERROR",
                    "message": msg,
                })),
            )
                .into_response())
        }
        Ok(Err(_)) => Err(json_error(
            StatusCode::BAD_GATEWAY,
            "FORWARDER_DISCONNECTED",
            "forwarder disconnected before replying",
        )),
        Err(_) => Err(gateway_timeout("forwarder did not respond within timeout")),
    }
}

fn reader_control_response_to_http(resp: ReaderControlResponse) -> axum::response::Response {
    if resp.success {
        Json(serde_json::json!({
            "ok": true,
            "error": serde_json::Value::Null,
            "reader_info": resp.reader_info,
        }))
        .into_response()
    } else {
        json_error(
            StatusCode::BAD_GATEWAY,
            "READER_CONTROL_ERROR",
            resp.error
                .unwrap_or_else(|| "reader control action failed".to_owned()),
        )
    }
}

/// Send a fire-and-forget command to a reader (sends command, returns 202 immediately).
async fn send_fire_and_forget(
    state: &AppState,
    forwarder_id: &str,
    reader_ip: &str,
    action: ReaderControlAction,
) -> axum::response::Response {
    if let Err(e) = validate_reader_ip(reader_ip) {
        return e.into_response();
    }
    tracing::debug!(forwarder_id = %forwarder_id, reader_ip = %reader_ip, action = ?action, "sending fire-and-forget reader control");

    let senders = state.forwarder_command_senders.read().await;
    let tx = match senders.get(forwarder_id) {
        Some(tx) => tx.clone(),
        None => return not_found("forwarder not connected"),
    };
    drop(senders);

    let cmd = ForwarderCommand::ReaderControlFireAndForget {
        reader_ip: reader_ip.to_owned(),
        action,
    };

    match tx.try_send(cmd) {
        Ok(()) => (
            StatusCode::ACCEPTED,
            Json(serde_json::json!({ "ok": true })),
        )
            .into_response(),
        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
            gateway_timeout("forwarder command queue is saturated")
        }
        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
            not_found("forwarder disconnected")
        }
    }
}

pub async fn get_reader_info(
    State(state): State<AppState>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    match send_reader_control(
        &state,
        &forwarder_id,
        &reader_ip,
        ReaderControlAction::GetInfo,
    )
    .await
    {
        Ok(resp) => reader_control_response_to_http(resp),
        Err(e) => e,
    }
}

pub async fn sync_clock(
    State(state): State<AppState>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    match send_reader_control(
        &state,
        &forwarder_id,
        &reader_ip,
        ReaderControlAction::SyncClock,
    )
    .await
    {
        Ok(resp) => reader_control_response_to_http(resp),
        Err(e) => e,
    }
}

#[derive(Deserialize)]
pub struct SetReadModeBody {
    pub mode: rt_protocol::ReadMode,
    pub timeout: u8,
}

pub async fn set_read_mode(
    State(state): State<AppState>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
    Json(body): Json<SetReadModeBody>,
) -> impl IntoResponse {
    match send_reader_control(
        &state,
        &forwarder_id,
        &reader_ip,
        ReaderControlAction::SetReadMode {
            mode: body.mode,
            timeout: body.timeout,
        },
    )
    .await
    {
        Ok(resp) => reader_control_response_to_http(resp),
        Err(e) => e,
    }
}

#[derive(Deserialize)]
pub struct SetTtoBody {
    pub enabled: bool,
}

pub async fn set_tto(
    State(state): State<AppState>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
    Json(body): Json<SetTtoBody>,
) -> impl IntoResponse {
    match send_reader_control(
        &state,
        &forwarder_id,
        &reader_ip,
        ReaderControlAction::SetTto {
            enabled: body.enabled,
        },
    )
    .await
    {
        Ok(resp) => reader_control_response_to_http(resp),
        Err(e) => e,
    }
}

#[derive(Deserialize)]
pub struct SetRecordingBody {
    pub enabled: bool,
}

pub async fn set_recording(
    State(state): State<AppState>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
    Json(body): Json<SetRecordingBody>,
) -> impl IntoResponse {
    match send_reader_control(
        &state,
        &forwarder_id,
        &reader_ip,
        ReaderControlAction::SetRecording {
            enabled: body.enabled,
        },
    )
    .await
    {
        Ok(resp) => reader_control_response_to_http(resp),
        Err(e) => e,
    }
}

pub async fn clear_records(
    State(state): State<AppState>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    send_fire_and_forget(
        &state,
        &forwarder_id,
        &reader_ip,
        ReaderControlAction::ClearRecords,
    )
    .await
}

pub async fn start_download(
    State(state): State<AppState>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    send_fire_and_forget(
        &state,
        &forwarder_id,
        &reader_ip,
        ReaderControlAction::StartDownload,
    )
    .await
}

pub async fn stop_download(
    State(state): State<AppState>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    match send_reader_control(
        &state,
        &forwarder_id,
        &reader_ip,
        ReaderControlAction::StopDownload,
    )
    .await
    {
        Ok(resp) => reader_control_response_to_http(resp),
        Err(e) => e,
    }
}

pub async fn refresh(
    State(state): State<AppState>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    match send_reader_control(
        &state,
        &forwarder_id,
        &reader_ip,
        ReaderControlAction::Refresh,
    )
    .await
    {
        Ok(resp) => reader_control_response_to_http(resp),
        Err(e) => e,
    }
}

pub async fn reconnect(
    State(state): State<AppState>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    match send_reader_control(
        &state,
        &forwarder_id,
        &reader_ip,
        ReaderControlAction::Reconnect,
    )
    .await
    {
        Ok(resp) => reader_control_response_to_http(resp),
        Err(e) => e,
    }
}

pub async fn get_all_reader_states(State(state): State<AppState>) -> impl IntoResponse {
    let cache = state.reader_states.read().await;
    let states: Vec<_> = cache.values().cloned().collect();
    Json(serde_json::json!({ "reader_states": states }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_reader_ip_accepts_valid_ip_port() {
        assert!(validate_reader_ip("10.0.0.1:10000").is_ok());
    }

    #[test]
    fn validate_reader_ip_rejects_bare_ip() {
        let err = validate_reader_ip("10.0.0.1").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validate_reader_ip_rejects_empty_string() {
        let err = validate_reader_ip("").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validate_reader_ip_rejects_trailing_colon() {
        let err = validate_reader_ip("10.0.0.1:").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn validate_reader_ip_rejects_leading_colon() {
        let err = validate_reader_ip(":10000").unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
    }
}
