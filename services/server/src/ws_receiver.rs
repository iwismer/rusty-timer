use axum::{extract::{ws::{Message, WebSocket, WebSocketUpgrade}, State}, http::HeaderMap, response::IntoResponse};
use rt_protocol::{error_codes, WsMessage};
use tracing::info;
use crate::{auth::{extract_bearer, validate_token}, state::AppState};

pub async fn ws_receiver_handler(ws: WebSocketUpgrade, State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let token = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| extract_bearer(v)).map(|s| s.to_owned());
    ws.on_upgrade(move |socket| handle_receiver_socket(socket, state, token))
}

async fn send_ws_error(socket: &mut WebSocket, code: &str, message: &str, retryable: bool) {
    let msg = WsMessage::Error(rt_protocol::ErrorMessage { code: code.to_owned(), message: message.to_owned(), retryable });
    if let Ok(json) = serde_json::to_string(&msg) { let _ = socket.send(Message::Text(json.into())).await; }
}

async fn handle_receiver_socket(mut socket: WebSocket, state: AppState, token: Option<String>) {
    let token_str = match token {
        Some(t) => t,
        None => { send_ws_error(&mut socket, error_codes::INVALID_TOKEN, "missing Authorization header", false).await; return; }
    };
    let claims = match validate_token(&state.pool, &token_str).await {
        Some(c) => c,
        None => { send_ws_error(&mut socket, error_codes::INVALID_TOKEN, "unknown or revoked token", false).await; return; }
    };
    if claims.device_type != "receiver" {
        send_ws_error(&mut socket, error_codes::INVALID_TOKEN, "token is not for a receiver device", false).await; return;
    }
    let device_id = claims.device_id.clone();
    info!(device_id = %device_id, "receiver connected (Task 10 implementation pending)");
    drop(socket);
}
