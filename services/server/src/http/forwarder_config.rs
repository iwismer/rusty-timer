use super::response::{bad_request, gateway_timeout, json_error, not_found};
use crate::state::{AppState, ForwarderCommand, ForwarderProxyReply};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use std::time::Duration;

const CONFIG_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const RESTART_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub async fn get_forwarder_config(
    State(state): State<AppState>,
    Path(forwarder_id): Path<String>,
) -> impl IntoResponse {
    let senders = state.forwarder_command_senders.read().await;
    let tx = match senders.get(&forwarder_id) {
        Some(tx) => tx.clone(),
        None => return not_found("forwarder not connected"),
    };
    drop(senders);

    let request_id = uuid::Uuid::new_v4().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ForwarderCommand::ConfigGet {
        request_id,
        reply: reply_tx,
    };

    match tokio::time::timeout(CONFIG_REQUEST_TIMEOUT, tx.send(cmd)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            return not_found("forwarder disconnected");
        }
        Err(_) => {
            return gateway_timeout("forwarder command queue is saturated");
        }
    }

    match tokio::time::timeout(CONFIG_REQUEST_TIMEOUT, reply_rx).await {
        Ok(Ok(ForwarderProxyReply::Response(resp))) => {
            if resp.ok {
                Json(serde_json::json!({
                    "ok": true,
                    "error": serde_json::Value::Null,
                    "config": resp.config,
                    "restart_needed": resp.restart_needed,
                }))
                .into_response()
            } else {
                json_error(
                    StatusCode::BAD_GATEWAY,
                    "FORWARDER_CONFIG_ERROR",
                    resp.error
                        .unwrap_or_else(|| "forwarder failed to read config".to_owned()),
                )
            }
        }
        Ok(Ok(ForwarderProxyReply::Timeout)) => {
            gateway_timeout("forwarder did not respond within timeout")
        }
        Ok(Err(_)) => json_error(
            StatusCode::BAD_GATEWAY,
            "FORWARDER_DISCONNECTED",
            "forwarder disconnected before replying",
        ),
        Err(_) => gateway_timeout("forwarder did not respond within timeout"),
    }
}

pub async fn set_forwarder_config(
    State(state): State<AppState>,
    Path((forwarder_id, section)): Path<(String, String)>,
    Json(payload): Json<serde_json::Value>,
) -> impl IntoResponse {
    send_config_set_command(&state, &forwarder_id, section, payload).await
}

pub async fn control_forwarder(
    State(state): State<AppState>,
    Path((forwarder_id, action)): Path<(String, String)>,
) -> impl IntoResponse {
    let action_value = match action.as_str() {
        "restart-service" => {
            return restart_forwarder(State(state), Path(forwarder_id))
                .await
                .into_response();
        }
        "restart-device" => "restart_device",
        "shutdown-device" => "shutdown_device",
        _ => return bad_request("unknown control action"),
    };
    let payload = serde_json::json!({ "action": action_value });
    send_config_set_command(&state, &forwarder_id, "control".to_owned(), payload).await
}

async fn send_config_set_command(
    state: &AppState,
    forwarder_id: &str,
    section: String,
    payload: serde_json::Value,
) -> axum::response::Response {
    let log_section = section.clone();
    let senders = state.forwarder_command_senders.read().await;
    let tx = match senders.get(forwarder_id) {
        Some(tx) => tx.clone(),
        None => return not_found("forwarder not connected"),
    };
    drop(senders);

    let request_id = uuid::Uuid::new_v4().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ForwarderCommand::ConfigSet {
        request_id,
        section,
        payload,
        reply: reply_tx,
    };

    match tokio::time::timeout(CONFIG_REQUEST_TIMEOUT, tx.send(cmd)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            return not_found("forwarder disconnected");
        }
        Err(_) => {
            return gateway_timeout("forwarder command queue is saturated");
        }
    }

    match tokio::time::timeout(CONFIG_REQUEST_TIMEOUT, reply_rx).await {
        Ok(Ok(ForwarderProxyReply::Response(resp))) => {
            let status = if resp.ok {
                state.logger.log(format!(
                    "forwarder \"{forwarder_id}\" config/{log_section} updated"
                ));
                StatusCode::OK
            } else {
                match resp
                    .status_code
                    .and_then(|code| StatusCode::from_u16(code).ok())
                {
                    Some(code) if code.is_client_error() => code,
                    Some(code) if code.is_server_error() => StatusCode::BAD_GATEWAY,
                    Some(_) | None => StatusCode::BAD_REQUEST,
                }
            };
            (
                status,
                Json(serde_json::json!({
                    "ok": resp.ok,
                    "error": resp.error,
                    "restart_needed": resp.restart_needed,
                    "status_code": resp.status_code,
                })),
            )
                .into_response()
        }
        Ok(Ok(ForwarderProxyReply::Timeout)) => {
            gateway_timeout("forwarder did not respond within timeout")
        }
        Ok(Err(_)) => json_error(
            StatusCode::BAD_GATEWAY,
            "FORWARDER_DISCONNECTED",
            "forwarder disconnected before replying",
        ),
        Err(_) => gateway_timeout("forwarder did not respond within timeout"),
    }
}

pub async fn restart_forwarder(
    State(state): State<AppState>,
    Path(forwarder_id): Path<String>,
) -> impl IntoResponse {
    let senders = state.forwarder_command_senders.read().await;
    let tx = match senders.get(&forwarder_id) {
        Some(tx) => tx.clone(),
        None => return not_found("forwarder not connected"),
    };
    drop(senders);

    let request_id = uuid::Uuid::new_v4().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let cmd = ForwarderCommand::Restart {
        request_id,
        reply: reply_tx,
    };

    match tokio::time::timeout(RESTART_REQUEST_TIMEOUT, tx.send(cmd)).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            return not_found("forwarder disconnected");
        }
        Err(_) => {
            return gateway_timeout("forwarder command queue is saturated");
        }
    }

    match tokio::time::timeout(RESTART_REQUEST_TIMEOUT, reply_rx).await {
        Ok(Ok(ForwarderProxyReply::Response(resp))) => {
            let status = if resp.ok {
                state
                    .logger
                    .log(format!("forwarder \"{forwarder_id}\" restart requested"));
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            };
            (
                status,
                Json(serde_json::json!({
                    "ok": resp.ok,
                    "error": resp.error,
                })),
            )
                .into_response()
        }
        Ok(Ok(ForwarderProxyReply::Timeout)) => {
            gateway_timeout("forwarder did not respond within timeout")
        }
        Ok(Err(_)) => json_error(
            StatusCode::BAD_GATEWAY,
            "FORWARDER_DISCONNECTED",
            "forwarder disconnected before replying",
        ),
        Err(_) => gateway_timeout("forwarder did not respond within timeout"),
    }
}
