use crate::auth::extract_bearer;
use axum::{
    extract::ws::{Message, WebSocket},
    http::HeaderMap,
};
use rt_protocol::{ErrorMessage, Heartbeat, WsMessage};
use std::time::Duration;

pub fn extract_token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(extract_bearer)
        .map(ToOwned::to_owned)
}

fn error_json(code: &str, message: &str, retryable: bool) -> Option<String> {
    serde_json::to_string(&WsMessage::Error(ErrorMessage {
        code: code.to_owned(),
        message: message.to_owned(),
        retryable,
    }))
    .ok()
}

pub async fn send_ws_error(socket: &mut WebSocket, code: &str, message: &str, retryable: bool) {
    if let Some(json) = error_json(code, message, retryable) {
        let _ = socket.send(Message::Text(json.into())).await;
    }
}

fn heartbeat_json(session_id: &str, device_id: &str) -> Option<String> {
    serde_json::to_string(&WsMessage::Heartbeat(Heartbeat {
        session_id: session_id.to_owned(),
        device_id: device_id.to_owned(),
    }))
    .ok()
}

pub async fn send_heartbeat(socket: &mut WebSocket, session_id: &str, device_id: &str) -> bool {
    if let Some(json) = heartbeat_json(session_id, device_id) {
        return socket.send(Message::Text(json.into())).await.is_ok();
    }
    true
}

fn parse_text_message(msg: Option<Result<Message, axum::Error>>) -> Result<String, ()> {
    match msg {
        Some(Ok(Message::Text(text))) => Ok(text.to_string()),
        _ => Err(()),
    }
}

pub async fn recv_text_with_timeout(
    socket: &mut WebSocket,
    timeout: Duration,
) -> Result<String, ()> {
    match tokio::time::timeout(timeout, socket.recv()).await {
        Ok(msg) => parse_text_message(msg),
        Err(_) => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_token_from_headers_handles_missing_malformed_and_valid_bearer() {
        let missing = HeaderMap::new();
        assert_eq!(extract_token_from_headers(&missing), None);

        let mut malformed = HeaderMap::new();
        malformed.insert(
            "authorization",
            axum::http::HeaderValue::from_static("Basic abc123"),
        );
        assert_eq!(extract_token_from_headers(&malformed), None);

        let mut valid = HeaderMap::new();
        valid.insert(
            "authorization",
            axum::http::HeaderValue::from_static("Bearer token-123"),
        );
        assert_eq!(
            extract_token_from_headers(&valid),
            Some("token-123".to_owned())
        );
    }

    #[test]
    fn recv_text_with_timeout_helper_accepts_text_hello_and_supports_parsing() {
        let hello = WsMessage::ForwarderHello(rt_protocol::ForwarderHello {
            forwarder_id: "fwd-1".to_owned(),
            reader_ips: vec!["10.0.0.1:10000".to_owned()],
            display_name: None,
        });
        let hello_json = serde_json::to_string(&hello).expect("serialize hello");

        let parsed = parse_text_message(Some(Ok(Message::Text(hello_json.clone()))))
            .expect("text message should be returned");
        match serde_json::from_str::<WsMessage>(&parsed).expect("hello JSON should parse") {
            WsMessage::ForwarderHello(_) => {}
            other => panic!("expected forwarder_hello, got {other:?}"),
        }
    }

    #[test]
    fn recv_text_with_timeout_helper_handles_timeout_and_malformed_paths() {
        assert_eq!(parse_text_message(None), Err(()));
        assert_eq!(
            parse_text_message(Some(Ok(Message::Ping(vec![1, 2].into())))),
            Err(())
        );

        let malformed = parse_text_message(Some(Ok(Message::Text("{not-json".into()))))
            .expect("text message still returns as text");
        assert!(
            serde_json::from_str::<WsMessage>(&malformed).is_err(),
            "malformed hello text should fail caller-side JSON parsing"
        );
    }

    #[test]
    fn send_ws_error_payload_serialization_matches_contract() {
        let text = error_json("PROTOCOL_ERROR", "expected forwarder_hello", false)
            .expect("error payload should serialize");
        let msg: WsMessage = serde_json::from_str(&text).expect("error payload should parse");
        assert_eq!(
            msg,
            WsMessage::Error(ErrorMessage {
                code: "PROTOCOL_ERROR".to_owned(),
                message: "expected forwarder_hello".to_owned(),
                retryable: false,
            })
        );
    }

    #[test]
    fn send_heartbeat_payload_serialization_matches_contract() {
        let text =
            heartbeat_json("session-1", "device-9").expect("heartbeat payload should serialize");
        let msg: WsMessage = serde_json::from_str(&text).expect("heartbeat payload should parse");
        assert_eq!(
            msg,
            WsMessage::Heartbeat(Heartbeat {
                session_id: "session-1".to_owned(),
                device_id: "device-9".to_owned(),
            })
        );
    }
}
