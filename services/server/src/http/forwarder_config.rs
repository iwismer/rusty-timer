use crate::state::{AppState, ForwarderCommand};
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

    if tx.send(cmd).await.is_err() {
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

    match tokio::time::timeout(CONFIG_REQUEST_TIMEOUT, reply_rx).await {
        Ok(Ok(resp)) => Json(serde_json::json!({
            "config": resp.config,
            "restart_needed": resp.restart_needed,
        }))
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
    let cmd = ForwarderCommand::ConfigSet {
        request_id,
        section,
        payload,
        reply: reply_tx,
    };

    if tx.send(cmd).await.is_err() {
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

    match tokio::time::timeout(CONFIG_REQUEST_TIMEOUT, reply_rx).await {
        Ok(Ok(resp)) => {
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
                    "restart_needed": resp.restart_needed,
                })),
            )
                .into_response()
        }
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

    if tx.send(cmd).await.is_err() {
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

    match tokio::time::timeout(RESTART_REQUEST_TIMEOUT, reply_rx).await {
        Ok(Ok(resp)) => {
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
