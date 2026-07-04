//! Headless test bridge — a loopback-only HTTP surface for driving the
//! receiver from automated agents/tests.
//!
//! This entire module is gated behind the `test-bridge` Cargo feature and is
//! compiled out of release/default builds. It is mounted only by the headless
//! host (`headless::control_router`), which enforces a loopback bind before any
//! routes are served, so the bridge is never reachable off-host.
//!
//! Contract:
//!   * `POST /bridge/invoke/:cmd` — dispatch a canonical control command. The
//!     request body is a JSON object whose keys match the command's argument
//!     names (see [`crate::control_api::receiver_command_list!`]). The success
//!     body is the command's JSON return value.
//!   * `GET  /bridge/events`      — Server-Sent Events stream of
//!     [`ReceiverUiEvent`], using [`control_api::event_name`] as the SSE event
//!     name and the JSON-serialized event as data.
//!   * `GET  /bridge/state`       — JSON view-model snapshot (status + streams).

use crate::control_api::{self, AppState};
use crate::error::ReceiverError;
use crate::ui_events::ReceiverUiEvent;
use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::Stream;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;

/// Build the `/bridge/*` router. Crate-private: only the headless host mounts
/// it, which guarantees the loopback-only invariant.
pub(crate) fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/bridge/invoke/{cmd}", post(invoke))
        .route("/bridge/events", get(events))
        .route("/bridge/state", get(state_snapshot))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failure modes when dispatching a bridge invocation.
enum BridgeError {
    /// The command name is not in the canonical registry (or is not wired into
    /// the bridge dispatch table).
    Unknown(String),
    /// The JSON args could not be deserialized into the command's parameters.
    BadArgs(String),
    /// The underlying control-API handler returned an error.
    Handler(ReceiverError),
}

impl From<ReceiverError> for BridgeError {
    fn from(e: ReceiverError) -> Self {
        BridgeError::Handler(e)
    }
}

impl IntoResponse for BridgeError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            BridgeError::Unknown(cmd) => (StatusCode::NOT_FOUND, format!("unknown command: {cmd}")),
            BridgeError::BadArgs(msg) => (StatusCode::BAD_REQUEST, msg),
            BridgeError::Handler(e) => (receiver_error_status(&e), e.to_string()),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

fn receiver_error_status(e: &ReceiverError) -> StatusCode {
    match e {
        ReceiverError::NotFound(_) => StatusCode::NOT_FOUND,
        ReceiverError::BadRequest(_) => StatusCode::BAD_REQUEST,
        ReceiverError::UpstreamError(_) => StatusCode::BAD_GATEWAY,
        ReceiverError::NotConnected(_) => StatusCode::CONFLICT,
        ReceiverError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

// ---------------------------------------------------------------------------
// Argument extraction
// ---------------------------------------------------------------------------

/// Deserialize a single named argument from the request's JSON object.
///
/// A missing key is treated as JSON `null`, so `Option<_>` arguments may be
/// omitted while required arguments produce a `BadArgs` error.
fn arg<T: DeserializeOwned>(args: &Value, name: &str) -> Result<T, BridgeError> {
    let value = args.get(name).cloned().unwrap_or(Value::Null);
    serde_json::from_value(value)
        .map_err(|e| BridgeError::BadArgs(format!("invalid argument `{name}`: {e}")))
}

/// Serialize a command's success value into a JSON body.
fn ok<T: serde::Serialize>(value: T) -> Result<Value, BridgeError> {
    serde_json::to_value(value)
        .map_err(|e| BridgeError::Handler(ReceiverError::Internal(e.to_string())))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn invoke(
    Path(cmd): Path<String>,
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> Response {
    // Reject commands not present in the canonical registry up front, so the
    // registry remains the single gatekeeper for the bridge surface.
    if control_api::command_spec(&cmd).is_none() {
        return BridgeError::Unknown(cmd).into_response();
    }

    let args: Value = if body.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        match serde_json::from_slice(&body) {
            Ok(v @ Value::Object(_)) => v,
            Ok(_) => {
                return BridgeError::BadArgs("request body must be a JSON object".to_owned())
                    .into_response();
            }
            Err(e) => {
                return BridgeError::BadArgs(format!("invalid JSON body: {e}")).into_response();
            }
        }
    };

    match dispatch(state.as_ref(), &cmd, &args).await {
        Ok(value) => (StatusCode::OK, Json(value)).into_response(),
        Err(err) => err.into_response(),
    }
}

async fn state_snapshot(State(state): State<Arc<AppState>>) -> Json<Value> {
    let status = control_api::get_status(state.as_ref()).await;
    let streams = local_streams_snapshot(state.as_ref()).await;
    Json(serde_json::json!({
        "status": status,
        "streams": streams,
    }))
}

async fn local_streams_snapshot(state: &AppState) -> control_api::StreamsResponse {
    state.build_streams_response().await
}

async fn events(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let tx = state.ui.ui_tx.clone();
    let rx = tx.subscribe();
    let stream = futures_util::stream::unfold((tx, rx), |(tx, mut rx)| async move {
        match rx.recv().await {
            Ok(event) => Some((Ok(sse_event(&event)), (tx, rx))),
            // On lag, ask the client to resync and reset to the live tail.
            // Tokio broadcast advances a lagged receiver to the oldest retained
            // event; continuing with it would deliver stale pre-resync deltas.
            Err(RecvError::Lagged(_)) => {
                let event = ReceiverUiEvent::Resync;
                let rx = tx.subscribe();
                Some((Ok(sse_event(&event)), (tx, rx)))
            }
            // Sender dropped: end the stream cleanly.
            Err(RecvError::Closed) => None,
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn sse_event(event: &ReceiverUiEvent) -> Event {
    let name = control_api::event_name(event);
    let data = serde_json::to_string(event).unwrap_or_else(|e| {
        tracing::warn!(error = %e, event = name, "failed to serialize bridge SSE event");
        "{}".to_owned()
    });
    Event::default().event(name).data(data)
}

// ---------------------------------------------------------------------------
// Command dispatch
//
// Every arm dispatches to a canonical `control_api` function. The set of arms
// mirrors `receiver_command_list!`; `dispatch_table_covers_registry` guards
// against drift (a registry command with no arm here yields `Unknown`).
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_lines)]
async fn dispatch(state: &AppState, cmd: &str, args: &Value) -> Result<Value, BridgeError> {
    match cmd {
        "get_profile" => ok(control_api::get_profile(state).await?),
        "put_profile" => ok(control_api::put_profile(state, arg(args, "body")?).await?),
        "get_mode" => ok(control_api::get_mode(state).await?),
        "put_mode" => ok(control_api::put_mode(state, arg(args, "mode")?).await?),
        "get_streams" => ok(control_api::get_streams(state).await),
        "get_stream_metrics" => ok(control_api::get_stream_metrics(state).await),
        "put_earliest_epoch" => {
            ok(control_api::put_earliest_epoch(state, arg(args, "body")?).await?)
        }
        "get_replay_target_epochs" => ok(control_api::get_replay_target_epochs(
            state,
            arg(args, "forwarder_endpoint_id")?,
            arg(args, "stream_id")?,
        )
        .await?),
        "get_subscriptions" => ok(control_api::get_subscriptions(state).await?),
        "put_subscriptions" => ok(control_api::put_subscriptions(state, arg(args, "body")?).await?),
        "get_status" => ok(control_api::get_status(state).await),
        "get_connections" => ok(control_api::get_connections(state).await),
        "reconnect_server" => ok(control_api::reconnect_server(state).await?),
        "connect_forwarder" => {
            ok(control_api::connect_forwarder(state, arg(args, "endpoint_id")?).await?)
        }
        "disconnect_forwarder" => {
            ok(control_api::disconnect_forwarder(state, arg(args, "endpoint_id")?).await?)
        }
        "reconnect_forwarder" => {
            ok(control_api::reconnect_forwarder(state, arg(args, "endpoint_id")?).await?)
        }
        "get_forwarder_config" => {
            ok(control_api::get_forwarder_config(state, arg(args, "endpoint_id")?).await?)
        }
        "set_forwarder_config" => ok(control_api::set_forwarder_config(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "config_json")?,
        )
        .await?),
        "restart_forwarder" => {
            ok(control_api::restart_forwarder(state, arg(args, "endpoint_id")?).await?)
        }
        "reader_get_info" => ok(control_api::reader_get_info(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
        )
        .await?),
        "reader_sync_clock" => ok(control_api::reader_sync_clock(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
        )
        .await?),
        "reader_set_epoch_name" => ok(control_api::reader_set_epoch_name(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
            arg(args, "name")?,
        )
        .await?),
        "reader_advance_epoch" => ok(control_api::reader_advance_epoch(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
        )
        .await?),
        "reader_set_read_mode" => ok(control_api::reader_set_read_mode(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
            arg(args, "mode")?,
            arg(args, "timeout")?,
        )
        .await?),
        "reader_set_tto" => ok(control_api::reader_set_tto(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
            arg(args, "enabled")?,
        )
        .await?),
        "reader_set_recording" => ok(control_api::reader_set_recording(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
            arg(args, "enabled")?,
        )
        .await?),
        "reader_clear_records" => ok(control_api::reader_clear_records(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
        )
        .await?),
        "reader_start_download" => ok(control_api::reader_start_download(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
        )
        .await?),
        "reader_stop_download" => ok(control_api::reader_stop_download(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
        )
        .await?),
        "reader_refresh" => ok(control_api::reader_refresh(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
        )
        .await?),
        "reader_reconnect" => ok(control_api::reader_reconnect(
            state,
            arg(args, "endpoint_id")?,
            arg(args, "stream_id")?,
        )
        .await?),
        "get_version" => ok(control_api::get_version()),
        "get_logs" => ok(control_api::get_logs(state).await),
        "admin_reset_cursor" => {
            ok(control_api::admin_reset_cursor(state, arg(args, "body")?).await?)
        }
        "admin_reset_all_cursors" => ok(control_api::admin_reset_all_cursors(state).await?),
        "admin_reset_earliest_epoch" => {
            ok(control_api::admin_reset_earliest_epoch(state, arg(args, "body")?).await?)
        }
        "admin_reset_all_earliest_epochs" => {
            ok(control_api::admin_reset_all_earliest_epochs(state).await?)
        }
        "admin_purge_subscriptions" => ok(control_api::admin_purge_subscriptions(state).await?),
        "admin_update_port" => ok(control_api::admin_update_port(state, arg(args, "body")?).await?),
        "admin_reset_profile" => ok(control_api::admin_reset_profile(state).await?),
        "admin_clear_data" => ok(control_api::admin_clear_data(state).await?),
        "admin_factory_reset" => ok(control_api::admin_factory_reset(state).await?),
        "get_dbf_config" => ok(control_api::get_dbf_config(state).await?),
        "put_dbf_config" => ok(control_api::put_dbf_config(state, arg(args, "body")?).await?),
        "clear_dbf" => ok(control_api::clear_dbf(state).await?),
        "update_subscription_event_type" => {
            let forwarder_endpoint_id: String = arg(args, "forwarder_endpoint_id")?;
            let stream_id: String = arg(args, "stream_id")?;
            ok(control_api::update_subscription_event_type(
                state,
                &forwarder_endpoint_id,
                &stream_id,
                arg(args, "body")?,
            )
            .await?)
        }
        "import_participants" => {
            ok(control_api::import_participants(state, arg(args, "contents")?).await?)
        }
        "import_chips" => ok(control_api::import_chips(state, arg(args, "contents")?).await?),
        "import_participants_file" => {
            ok(control_api::import_participants_file(state, arg(args, "path")?).await?)
        }
        "import_chips_file" => ok(control_api::import_chips_file(state, arg(args, "path")?).await?),
        "import_participants_from_rd" => {
            ok(control_api::import_participants_from_rd(state, arg(args, "dir")?).await?)
        }
        "get_rd_import_config" => ok(control_api::get_rd_import_config(state).await?),
        "put_rd_import_config" => {
            ok(control_api::put_rd_import_config(state, arg(args, "body")?).await?)
        }
        "get_data_stats" => ok(control_api::get_data_stats(state).await?),
        "set_announcer_enabled" => {
            ok(control_api::set_announcer_enabled(state, arg(args, "enabled")?).await?)
        }
        "set_announcer_max_list_size" => ok(control_api::set_announcer_max_list_size(
            state,
            arg(args, "max_list_size")?,
        )
        .await?),
        "set_stream_announcer_publish" => {
            let forwarder_endpoint_id: String = arg(args, "forwarder_endpoint_id")?;
            let stream_id: String = arg(args, "stream_id")?;
            ok(control_api::set_stream_announcer_publish(
                state,
                &forwarder_endpoint_id,
                &stream_id,
                arg(args, "publish")?,
            )
            .await?)
        }
        other => Err(BridgeError::Unknown(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::ui_events::ReceiverUiEvent;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_bridge() -> (std::net::SocketAddr, Arc<AppState>) {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let app = router(Arc::clone(&state));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (addr, state)
    }

    #[tokio::test]
    async fn invoke_get_status_via_http() {
        let (addr, _state) = spawn_bridge().await;
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/bridge/invoke/get_status"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .expect("POST /bridge/invoke/get_status");
        assert!(resp.status().is_success(), "status: {}", resp.status());
        let body: serde_json::Value = resp.json().await.expect("status json");
        assert_eq!(body["receiver_id"], "recv-test");
        assert_eq!(body["connection_state"], "disconnected");
        assert_eq!(body["local_ok"], true);
        assert_eq!(body["streams_count"], 0);
    }

    /// Drift guard: every command in the canonical registry must be wired into
    /// the bridge dispatch table. Calling each with empty args must never
    /// produce `BridgeError::Unknown` (it may legitimately produce `BadArgs`
    /// for required arguments or `Handler` errors for unmet preconditions).
    ///
    /// The seeded DB keeps the dispatch side-effect-free: an unparseable
    /// `server_url` makes network-backed commands fail fast with no I/O.
    #[tokio::test]
    async fn dispatch_table_covers_registry() {
        for name in control_api::bridge_command_names() {
            let mut db = Db::open_in_memory().unwrap();
            db.save_profile(
                "invalid",
                "token",
                crate::db::DEFAULT_UPDATE_MODE,
                Some("recv-test"),
            )
            .unwrap();
            db.save_dbf_config(&crate::db::DbfConfig {
                enabled: false,
                flush_interval_ms: crate::db::DEFAULT_DBF_FLUSH_INTERVAL_MS,
            })
            .unwrap();
            let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
            let args = Value::Object(serde_json::Map::new());
            if let Err(BridgeError::Unknown(cmd)) = dispatch(state.as_ref(), name, &args).await {
                panic!("registry command `{cmd}` is not wired into the bridge dispatch table");
            }
        }
    }

    #[tokio::test]
    async fn invoke_update_subscription_event_type_uses_stream_identity_args() {
        let mut db = Db::open_in_memory().unwrap();
        db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
            forwarder_endpoint_id: "endpoint-1".to_owned(),
            stream_id: "55555555-5555-5555-5555-555555555555".to_owned(),
            local_port_override: None,
            event_type: crate::db::EventType::Finish,
            forwarder_id: Some("legacy-fwd".to_owned()),
            reader_ip: Some("10.0.0.1:10000".to_owned()),
        }])
        .unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let args = serde_json::json!({
            "forwarder_endpoint_id": "endpoint-1",
            "stream_id": "55555555-5555-5555-5555-555555555555",
            "body": { "event_type": "start" }
        });

        let result = dispatch(state.as_ref(), "update_subscription_event_type", &args).await;
        assert!(
            result.is_ok(),
            "update_subscription_event_type dispatch should accept stream identity args"
        );

        let db = state.storage.db.lock().await;
        let subs = db.load_stream_subscriptions().unwrap();
        assert_eq!(subs[0].event_type, crate::db::EventType::Start);
    }

    #[tokio::test]
    async fn invoke_unknown_command_is_404() {
        let (addr, _state) = spawn_bridge().await;
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/bridge/invoke/does_not_exist"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .expect("POST unknown command");
        assert_eq!(resp.status().as_u16(), 404);
    }

    #[tokio::test]
    async fn state_snapshot_uses_canonical_stream_port_resolution() {
        let (addr, state) = spawn_bridge().await;
        {
            let mut db = state.storage.db.lock().await;
            db.replace_stream_subscriptions(&[crate::db::StreamSubscription {
                forwarder_endpoint_id: "endpoint-default".to_owned(),
                stream_id: "10.0.0.5:10000".to_owned(),
                local_port_override: None,
                event_type: crate::db::EventType::Finish,
                forwarder_id: None,
                reader_ip: None,
            }])
            .unwrap();
        }

        let canonical = control_api::get_streams(state.as_ref()).await;
        let canonical_stream = canonical
            .streams
            .iter()
            .find(|s| s.forwarder_endpoint_id == "endpoint-default")
            .unwrap();
        assert_eq!(canonical_stream.local_port, Some(10005));
        assert_eq!(canonical_stream.local_port_override, None);

        let resp = reqwest::get(format!("http://{addr}/bridge/state"))
            .await
            .expect("GET /bridge/state");
        assert!(resp.status().is_success());
        let body: serde_json::Value = resp.json().await.expect("state json");
        let bridge_streams = body["streams"]["streams"].as_array().unwrap();
        let bridge_stream = bridge_streams
            .iter()
            .find(|s| s["forwarder_endpoint_id"] == "endpoint-default")
            .unwrap();
        assert_eq!(
            bridge_stream["local_port"],
            serde_json::json!(canonical_stream.local_port)
        );
        assert!(bridge_stream.get("local_port_override").is_some());
        assert_eq!(
            bridge_stream["local_port_override"],
            serde_json::Value::Null
        );
    }

    #[tokio::test]
    async fn state_snapshot() {
        let (addr, _state) = spawn_bridge().await;
        let resp = reqwest::get(format!("http://{addr}/bridge/state"))
            .await
            .expect("GET /bridge/state");
        assert!(resp.status().is_success());
        let body: serde_json::Value = resp.json().await.expect("state json");
        // Snapshot must include the current status view-model …
        assert_eq!(body["status"]["receiver_id"], "recv-test");
        assert_eq!(body["status"]["connection_state"], "disconnected");
        assert_eq!(body["status"]["local_ok"], true);
        // … and stream-related state.
        assert!(body["streams"]["streams"].is_array());
        assert!(body["streams"].get("degraded").is_some());
    }

    /// Open the SSE stream over a raw TCP connection, wait until the response
    /// headers are received (which proves the broadcast subscription is live),
    /// then return the connection so the caller can trigger an event.
    async fn open_sse(addr: std::net::SocketAddr) -> tokio::net::TcpStream {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let req = format!(
            "GET /bridge/events HTTP/1.1\r\nHost: {addr}\r\nAccept: text/event-stream\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        // Read until the end of the HTTP response headers.
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let n = stream.read(&mut byte).await.unwrap();
            assert!(n > 0, "connection closed before headers completed");
            buf.push(byte[0]);
            if buf.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        stream
    }

    #[tokio::test]
    async fn events_stream_emits() {
        let (addr, state) = spawn_bridge().await;
        let mut stream = open_sse(addr).await;

        // Subscription is now live; emit a deterministic event.
        let _ = state.ui.ui_tx.send(ReceiverUiEvent::LogEntry {
            entry: "bridge-hello".to_owned(),
        });

        // Read response bytes until we observe the SSE event, or time out.
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(!remaining.is_zero(), "timed out waiting for SSE event");
            let n = tokio::time::timeout(remaining, stream.read(&mut chunk))
                .await
                .expect("timed out waiting for SSE event")
                .expect("read SSE bytes");
            assert!(n > 0, "connection closed before SSE event arrived");
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf);
            if text.contains("event: log_entry") && text.contains("bridge-hello") {
                return;
            }
        }
    }

    async fn next_sse_frame_text(body: &mut axum::body::Body) -> String {
        use http_body_util::BodyExt;

        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
                .await
                .expect("timed out waiting for SSE frame")
                .expect("SSE stream ended before frame")
                .expect("SSE body error");
            match frame.into_data() {
                Ok(data) if !data.is_empty() => {
                    return String::from_utf8(data.to_vec()).expect("SSE frame is UTF-8");
                }
                _ => {}
            }
        }
    }

    #[tokio::test]
    async fn events_stream_resubscribes_after_lag() {
        let db = Db::open_in_memory().unwrap();
        let (state, _shutdown_rx) = AppState::new(db, "recv-test".to_owned());
        let mut response = events(State(Arc::clone(&state))).await.into_response();

        for i in 0..257 {
            let _ = state.ui.ui_tx.send(ReceiverUiEvent::LogEntry {
                entry: format!("old-retained-{i}"),
            });
        }

        let resync = next_sse_frame_text(response.body_mut()).await;
        assert!(resync.contains("event: resync"), "frame: {resync}");
        assert!(!resync.contains("old-retained-"), "frame: {resync}");

        let stale = {
            use futures_util::FutureExt;
            next_sse_frame_text(response.body_mut()).now_or_never()
        };
        assert!(stale.is_none(), "stale frame after resync: {stale:?}");

        let _ = state.ui.ui_tx.send(ReceiverUiEvent::LogEntry {
            entry: "live-after-resync".to_owned(),
        });

        let next = next_sse_frame_text(response.body_mut()).await;
        assert!(next.contains("event: log_entry"), "frame: {next}");
        assert!(next.contains("live-after-resync"), "frame: {next}");
        assert!(!next.contains("old-retained-"), "frame: {next}");
    }
}
