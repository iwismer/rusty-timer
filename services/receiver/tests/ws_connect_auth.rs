//! Tests that `build_authenticated_request` produces a properly-formed
//! WebSocket request and that the Authorization header actually reaches the
//! server.

use axum::{
    Router,
    extract::{
        State,
        ws::{WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::IntoResponse,
    routing::get,
};
use receiver::build_authenticated_request;
use std::sync::Arc;
use tokio::sync::Mutex;

// ── Unit tests ────────────────────────────────────────────────────────────────

/// The Authorization header must be present and carry the Bearer token.
#[test]
fn build_request_includes_bearer_token() {
    let req = build_authenticated_request("ws://127.0.0.1:9999/", "my-token").unwrap();
    let auth = req
        .headers()
        .get("authorization")
        .expect("authorization header missing");
    assert_eq!(auth.to_str().unwrap(), "Bearer my-token");
}

/// `IntoClientRequest` must populate the WebSocket upgrade headers that are
/// required by the protocol (`Sec-WebSocket-Key`, `Upgrade`).  If we used
/// `Request::builder()` directly these would be absent, which was the original
/// bug.
#[test]
fn build_request_preserves_ws_upgrade_headers() {
    let req = build_authenticated_request("ws://127.0.0.1:9999/", "my-token").unwrap();
    assert!(
        req.headers().get("sec-websocket-key").is_some(),
        "sec-websocket-key must be set by IntoClientRequest"
    );
    let upgrade = req
        .headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert_eq!(upgrade.to_ascii_lowercase(), "websocket");
}

/// Garbage input should surface as an error, not a panic.
/// A URL with a space is not a valid URI and must be rejected.
#[test]
fn build_request_rejects_invalid_url() {
    assert!(build_authenticated_request("not a valid url", "token").is_err());
}

// ── Integration test ──────────────────────────────────────────────────────────

/// Axum state that forwards the captured Authorization header value to the
/// waiting test assertion via a oneshot channel.
#[derive(Clone)]
struct Capture {
    tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<String>>>>,
}

async fn capture_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(cap): State<Capture>,
) -> impl IntoResponse {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    if let Some(tx) = cap.tx.lock().await.take() {
        let _ = tx.send(auth);
    }
    ws.on_upgrade(|_socket: WebSocket| async {})
}

/// End-to-end: the receiver connects to a real in-process WebSocket server and
/// the server receives exactly `Authorization: Bearer <token>`.
#[tokio::test]
async fn connect_sends_authorization_header() {
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let cap = Capture {
        tx: Arc::new(Mutex::new(Some(tx))),
    };

    let app = Router::new()
        .route("/ws", get(capture_handler))
        .with_state(cap);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("local_addr");

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });

    let url = format!("ws://{}/ws", addr);
    let req = build_authenticated_request(&url, "secret-token").expect("build request");
    tokio_tungstenite::connect_async(req)
        .await
        .expect("connect");

    let received = rx.await.expect("receive auth header");
    assert_eq!(
        received, "Bearer secret-token",
        "server must see the correct Bearer token"
    );
}
