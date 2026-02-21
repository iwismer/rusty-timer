use crate::state::{AppState, ForwarderCommand, ForwarderProxyReply};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use rt_protocol::HttpErrorEnvelope;
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
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(HttpErrorEnvelope {
                    code: "NOT_FOUND".to_owned(),
                    message: "forwarder not connected".to_owned(),
                    details: None,
                }),
            )
                .into_response()
        }
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
            return (
                StatusCode::NOT_FOUND,
                Json(HttpErrorEnvelope {
                    code: "NOT_FOUND".to_owned(),
                    message: "forwarder disconnected".to_owned(),
                    details: None,
                }),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(HttpErrorEnvelope {
                    code: "TIMEOUT".to_owned(),
                    message: "forwarder command queue is saturated".to_owned(),
                    details: None,
                }),
            )
                .into_response();
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
                (
                    StatusCode::BAD_GATEWAY,
                    Json(HttpErrorEnvelope {
                        code: "FORWARDER_CONFIG_ERROR".to_owned(),
                        message: resp
                            .error
                            .unwrap_or_else(|| "forwarder failed to read config".to_owned()),
                        details: None,
                    }),
                )
                    .into_response()
            }
        }
        Ok(Ok(ForwarderProxyReply::Timeout)) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(HttpErrorEnvelope {
                code: "TIMEOUT".to_owned(),
                message: "forwarder did not respond within timeout".to_owned(),
                details: None,
            }),
        )
            .into_response(),
        Ok(Err(_)) => (
            StatusCode::BAD_GATEWAY,
            Json(HttpErrorEnvelope {
                code: "FORWARDER_DISCONNECTED".to_owned(),
                message: "forwarder disconnected before replying".to_owned(),
                details: None,
            }),
        )
            .into_response(),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(HttpErrorEnvelope {
                code: "TIMEOUT".to_owned(),
                message: "forwarder did not respond within timeout".to_owned(),
                details: None,
            }),
        )
            .into_response(),
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
        "restart-service" => "restart_service",
        "restart-device" => "restart_device",
        "shutdown-device" => "shutdown_device",
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(HttpErrorEnvelope {
                    code: "BAD_REQUEST".to_owned(),
                    message: "unknown control action".to_owned(),
                    details: None,
                }),
            )
                .into_response()
        }
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
    let senders = state.forwarder_command_senders.read().await;
    let tx = match senders.get(forwarder_id) {
        Some(tx) => tx.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(HttpErrorEnvelope {
                    code: "NOT_FOUND".to_owned(),
                    message: "forwarder not connected".to_owned(),
                    details: None,
                }),
            )
                .into_response()
        }
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
            return (
                StatusCode::NOT_FOUND,
                Json(HttpErrorEnvelope {
                    code: "NOT_FOUND".to_owned(),
                    message: "forwarder disconnected".to_owned(),
                    details: None,
                }),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(HttpErrorEnvelope {
                    code: "TIMEOUT".to_owned(),
                    message: "forwarder command queue is saturated".to_owned(),
                    details: None,
                }),
            )
                .into_response();
        }
    }

    match tokio::time::timeout(CONFIG_REQUEST_TIMEOUT, reply_rx).await {
        Ok(Ok(ForwarderProxyReply::Response(resp))) => {
            let status = if resp.ok {
                StatusCode::OK
            } else if resp
                .status_code
                .is_some_and(|code| (400..500).contains(&code))
                || resp.status_code.is_none()
            {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::BAD_GATEWAY
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
        Ok(Ok(ForwarderProxyReply::Timeout)) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(HttpErrorEnvelope {
                code: "TIMEOUT".to_owned(),
                message: "forwarder did not respond within timeout".to_owned(),
                details: None,
            }),
        )
            .into_response(),
        Ok(Err(_)) => (
            StatusCode::BAD_GATEWAY,
            Json(HttpErrorEnvelope {
                code: "FORWARDER_DISCONNECTED".to_owned(),
                message: "forwarder disconnected before replying".to_owned(),
                details: None,
            }),
        )
            .into_response(),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(HttpErrorEnvelope {
                code: "TIMEOUT".to_owned(),
                message: "forwarder did not respond within timeout".to_owned(),
                details: None,
            }),
        )
            .into_response(),
    }
}

pub async fn restart_forwarder(
    State(state): State<AppState>,
    Path(forwarder_id): Path<String>,
) -> impl IntoResponse {
    let senders = state.forwarder_command_senders.read().await;
    let tx = match senders.get(&forwarder_id) {
        Some(tx) => tx.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(HttpErrorEnvelope {
                    code: "NOT_FOUND".to_owned(),
                    message: "forwarder not connected".to_owned(),
                    details: None,
                }),
            )
                .into_response()
        }
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
            return (
                StatusCode::NOT_FOUND,
                Json(HttpErrorEnvelope {
                    code: "NOT_FOUND".to_owned(),
                    message: "forwarder disconnected".to_owned(),
                    details: None,
                }),
            )
                .into_response();
        }
        Err(_) => {
            return (
                StatusCode::GATEWAY_TIMEOUT,
                Json(HttpErrorEnvelope {
                    code: "TIMEOUT".to_owned(),
                    message: "forwarder command queue is saturated".to_owned(),
                    details: None,
                }),
            )
                .into_response();
        }
    }

    match tokio::time::timeout(RESTART_REQUEST_TIMEOUT, reply_rx).await {
        Ok(Ok(ForwarderProxyReply::Response(resp))) => {
            let status = if resp.ok {
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
        Ok(Ok(ForwarderProxyReply::Timeout)) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(HttpErrorEnvelope {
                code: "TIMEOUT".to_owned(),
                message: "forwarder did not respond within timeout".to_owned(),
                details: None,
            }),
        )
            .into_response(),
        Ok(Err(_)) => (
            StatusCode::BAD_GATEWAY,
            Json(HttpErrorEnvelope {
                code: "FORWARDER_DISCONNECTED".to_owned(),
                message: "forwarder disconnected before replying".to_owned(),
                details: None,
            }),
        )
            .into_response(),
        Err(_) => (
            StatusCode::GATEWAY_TIMEOUT,
            Json(HttpErrorEnvelope {
                code: "TIMEOUT".to_owned(),
                message: "forwarder did not respond within timeout".to_owned(),
                details: None,
            }),
        )
            .into_response(),
    }
}
