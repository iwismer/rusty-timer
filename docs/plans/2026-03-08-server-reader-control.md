# Server-Side Reader Control Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Proxy IPICO reader control commands from the server dashboard through the forwarder WebSocket, so readers can be managed remotely.

**Architecture:** New WsMessage variants carry reader control requests/responses and unsolicited state pushes over the existing forwarder WS connection. The server exposes HTTP endpoints that proxy to the forwarder via the existing `ForwarderCommand` mpsc pattern. The dashboard UI adds expandable reader detail panels to stream cards.

**Tech Stack:** Rust (rt-protocol, server, forwarder), SvelteKit (server-ui, shared-ui), Axum HTTP, tokio WebSocket, SSE

---

### Task 1: Add ReaderInfo and ReaderControlAction types to rt-protocol

**Files:**
- Modify: `crates/rt-protocol/src/lib.rs`

**Step 1: Add the shared ReaderInfo types**

Add these structs after the existing config types (after line ~300). Follow the same derive pattern as other protocol types (`Debug, Clone, PartialEq, Eq, Serialize, Deserialize`).

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareInfo {
    pub fw_version: Option<String>,
    pub hw_code: Option<String>,
    pub reader_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config3Info {
    pub mode: String,
    pub timeout: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClockInfo {
    pub reader_clock: String,
    pub drift_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hardware: Option<HardwareInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Config3Info>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tto_enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<ClockInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_stored_reads: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<bool>,
    #[serde(default)]
    pub connect_failures: u8,
}
```

**Step 2: Add ReaderControlAction enum**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReaderControlAction {
    GetInfo,
    SyncClock,
    SetReadMode { mode: String, timeout: u8 },
    SetTto { enabled: bool },
    SetRecording { enabled: bool },
    ClearRecords,
    StartDownload,
    StopDownload,
    Refresh,
    Reconnect,
}
```

**Step 3: Add the WS message structs**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderControlRequest {
    pub request_id: String,
    pub reader_ip: String,
    pub action: ReaderControlAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderControlResponse {
    pub request_id: String,
    pub reader_ip: String,
    pub success: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderInfoUpdate {
    pub reader_ip: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderDownloadProgress {
    pub reader_ip: String,
    pub state: String,
    pub reads_received: u32,
    pub progress: u64,
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
```

**Step 4: Add variants to WsMessage enum**

Add these four variants to the `WsMessage` enum (after `RestartResponse`):

```rust
    ReaderControlRequest(ReaderControlRequest),
    ReaderControlResponse(ReaderControlResponse),
    ReaderInfoUpdate(ReaderInfoUpdate),
    ReaderDownloadProgress(ReaderDownloadProgress),
```

**Step 5: Verify it compiles**

Run: `cargo check -p rt-protocol`
Expected: compiles with no errors

**Step 6: Add a round-trip serde test**

Add a test in the existing test module of `lib.rs` (follow the pattern of existing WsMessage serde tests):

```rust
#[test]
fn reader_control_request_round_trip() {
    let msg = WsMessage::ReaderControlRequest(ReaderControlRequest {
        request_id: "abc".into(),
        reader_ip: "192.168.0.1:10000".into(),
        action: ReaderControlAction::SetReadMode {
            mode: "event".into(),
            timeout: 5,
        },
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn reader_control_response_round_trip() {
    let msg = WsMessage::ReaderControlResponse(ReaderControlResponse {
        request_id: "abc".into(),
        reader_ip: "192.168.0.1:10000".into(),
        success: true,
        error: None,
        reader_info: Some(ReaderInfo {
            banner: None,
            hardware: Some(HardwareInfo {
                fw_version: Some("3.09".into()),
                hw_code: Some("0x1234".into()),
                reader_id: Some("READER01".into()),
            }),
            config: Some(Config3Info { mode: "event".into(), timeout: 5 }),
            tto_enabled: Some(false),
            clock: Some(ClockInfo { reader_clock: "2026-03-08T12:00:00".into(), drift_ms: 42 }),
            estimated_stored_reads: Some(100),
            recording: Some(true),
            connect_failures: 0,
        }),
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn reader_info_update_round_trip() {
    let msg = WsMessage::ReaderInfoUpdate(ReaderInfoUpdate {
        reader_ip: "192.168.0.1:10000".into(),
        state: "connected".into(),
        reader_info: None,
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn reader_download_progress_round_trip() {
    let msg = WsMessage::ReaderDownloadProgress(ReaderDownloadProgress {
        reader_ip: "192.168.0.1:10000".into(),
        state: "downloading".into(),
        reads_received: 50,
        progress: 1024,
        total: 2048,
        error: None,
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}
```

**Step 7: Run the tests**

Run: `cargo test -p rt-protocol`
Expected: all tests pass

**Step 8: Commit**

```bash
git add crates/rt-protocol/src/lib.rs
git commit -m "feat(protocol): add reader control WsMessage types"
```

---

### Task 2: Add ReaderControl variant to server ForwarderCommand and state

**Files:**
- Modify: `services/server/src/state.rs`
- Modify: `services/server/src/dashboard_events.rs`

**Step 1: Add ReaderControl to ForwarderCommand**

In `services/server/src/state.rs`, add to the `ForwarderCommand` enum (after the `Restart` variant, around line 32):

```rust
    ReaderControl {
        request_id: String,
        reader_ip: String,
        action: rt_protocol::ReaderControlAction,
        reply: oneshot::Sender<ForwarderProxyReply<rt_protocol::ReaderControlResponse>>,
    },
```

**Step 2: Add reader state cache to AppState**

Add a new type and field to `AppState` in `state.rs`:

```rust
#[derive(Debug, Clone, Serialize)]
pub struct CachedReaderState {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub state: String,
    pub reader_info: Option<rt_protocol::ReaderInfo>,
}

pub type ReaderStateCache = Arc<RwLock<HashMap<String, CachedReaderState>>>;
```

The HashMap key is `"{forwarder_id}:{reader_ip}"`. Add the field to `AppState`:

```rust
    pub reader_states: ReaderStateCache,
```

Update the `AppState::new()` or construction site to initialize `reader_states: Arc::new(RwLock::new(HashMap::new()))`.

**Step 3: Add new DashboardEvent variants**

In `services/server/src/dashboard_events.rs`, add to the `DashboardEvent` enum:

```rust
    ReaderInfoUpdated {
        forwarder_id: String,
        reader_ip: String,
        state: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        reader_info: Option<rt_protocol::ReaderInfo>,
    },
    ReaderDownloadProgress {
        forwarder_id: String,
        reader_ip: String,
        state: String,
        reads_received: u32,
        progress: u64,
        total: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
```

**Step 4: Add SSE event type mappings**

In `services/server/src/http/sse.rs`, add match arms for the new `DashboardEvent` variants in the `event_type` match (around line 17-26):

```rust
DashboardEvent::ReaderInfoUpdated { .. } => "reader_info_updated",
DashboardEvent::ReaderDownloadProgress { .. } => "reader_download_progress",
```

**Step 5: Verify it compiles**

Run: `cargo check -p rt-server`
Expected: compiles (may have warnings about unused variants, which is fine at this stage)

**Step 6: Commit**

```bash
git add services/server/src/state.rs services/server/src/dashboard_events.rs services/server/src/http/sse.rs
git commit -m "feat(server): add reader control command, state cache, and dashboard events"
```

---

### Task 3: Handle reader control in server WS forwarder loop

**Files:**
- Modify: `services/server/src/ws_forwarder.rs`

**Step 1: Add pending reader control map**

Near the existing `pending_config_gets`, `pending_config_sets`, `pending_restarts` declarations (around line 302-322), add:

```rust
let mut pending_reader_controls: HashMap<String, (Instant, oneshot::Sender<ForwarderProxyReply<rt_protocol::ReaderControlResponse>>)> = HashMap::new();
```

**Step 2: Handle outbound ReaderControl command**

In the `cmd_rx` match arm (around line 458-503), add a new arm for `ForwarderCommand::ReaderControl`:

```rust
ForwarderCommand::ReaderControl { request_id, reader_ip, action, reply } => {
    let msg = WsMessage::ReaderControlRequest(rt_protocol::ReaderControlRequest {
        request_id: request_id.clone(),
        reader_ip,
        action,
    });
    let json = serde_json::to_string(&msg).unwrap();
    if let Err(e) = ws_sink.send(Message::Text(json.into())).await {
        warn!(error = %e, "failed to send reader control request");
        let _ = reply.send(ForwarderProxyReply::Timeout);
    } else {
        pending_reader_controls.insert(request_id, (Instant::now(), reply));
    }
}
```

**Step 3: Handle inbound ReaderControlResponse**

In the inbound WS message match (around line 330-450), add arms for the new message types:

```rust
WsMessage::ReaderControlResponse(resp) => {
    if let Some((_, reply)) = pending_reader_controls.remove(&resp.request_id) {
        let _ = reply.send(ForwarderProxyReply::Response(resp));
    }
}
WsMessage::ReaderInfoUpdate(update) => {
    let key = format!("{}:{}", device_id, update.reader_ip);
    let cached = CachedReaderState {
        forwarder_id: device_id.clone(),
        reader_ip: update.reader_ip.clone(),
        state: update.state.clone(),
        reader_info: update.reader_info.clone(),
    };
    state.reader_states.write().await.insert(key, cached);
    let _ = state.dashboard_tx.send(DashboardEvent::ReaderInfoUpdated {
        forwarder_id: device_id.clone(),
        reader_ip: update.reader_ip,
        state: update.state,
        reader_info: update.reader_info,
    });
}
WsMessage::ReaderDownloadProgress(progress) => {
    let _ = state.dashboard_tx.send(DashboardEvent::ReaderDownloadProgress {
        forwarder_id: device_id.clone(),
        reader_ip: progress.reader_ip,
        state: progress.state,
        reads_received: progress.reads_received,
        progress: progress.progress,
        total: progress.total,
        error: progress.error,
    });
}
```

**Step 4: Add pending_reader_controls to expiry**

Add `expire_pending_requests(&mut pending_reader_controls, now, FORWARDER_COMMAND_TIMEOUT);` alongside the existing expiry calls (around line 325-327 and 453-455).

**Step 5: Clear reader state cache on disconnect**

In the disconnect cleanup section (around line 508-526), add:

```rust
{
    let mut cache = state.reader_states.write().await;
    let keys_to_remove: Vec<String> = cache.keys()
        .filter(|k| k.starts_with(&format!("{}:", device_id)))
        .cloned()
        .collect();
    for key in &keys_to_remove {
        if let Some(cached) = cache.remove(key) {
            let _ = state.dashboard_tx.send(DashboardEvent::ReaderInfoUpdated {
                forwarder_id: device_id.clone(),
                reader_ip: cached.reader_ip,
                state: "disconnected".into(),
                reader_info: None,
            });
        }
    }
}
```

**Step 6: Verify it compiles**

Run: `cargo check -p rt-server`
Expected: compiles

**Step 7: Commit**

```bash
git add services/server/src/ws_forwarder.rs
git commit -m "feat(server): handle reader control messages in WS forwarder loop"
```

---

### Task 4: Add server HTTP endpoints for reader control

**Files:**
- Create: `services/server/src/http/reader_control.rs`
- Modify: `services/server/src/http/mod.rs`
- Modify: `services/server/src/lib.rs` (route registration)

**Step 1: Create the reader control HTTP module**

Create `services/server/src/http/reader_control.rs`. Follow the exact pattern from `forwarder_config.rs` (lines 14-71) for the proxy flow. The core helper:

```rust
use axum::extract::{Path, State, Json};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::state::{AppState, ForwarderCommand, ForwarderProxyReply};
use crate::http::response::ErrorResponse;
use rt_protocol::{ReaderControlAction, ReaderControlResponse};

const READER_CONTROL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

async fn send_reader_control(
    state: &AppState,
    forwarder_id: &str,
    reader_ip: &str,
    action: ReaderControlAction,
) -> Result<ReaderControlResponse, impl IntoResponse> {
    let senders = state.forwarder_command_senders.read().await;
    let Some(tx) = senders.get(forwarder_id).cloned() else {
        return Err((StatusCode::NOT_FOUND, "forwarder not connected").into_response());
    };
    drop(senders);

    let request_id = Uuid::new_v4().to_string();
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();

    let cmd = ForwarderCommand::ReaderControl {
        request_id,
        reader_ip: reader_ip.to_string(),
        action,
        reply: reply_tx,
    };

    if tx.send(cmd).await.is_err() {
        return Err((StatusCode::BAD_GATEWAY, "forwarder disconnected").into_response());
    }

    match tokio::time::timeout(READER_CONTROL_TIMEOUT, reply_rx).await {
        Ok(Ok(ForwarderProxyReply::Response(resp))) => {
            if resp.success {
                Ok(resp)
            } else {
                let msg = resp.error.unwrap_or_else(|| "unknown error".into());
                Err((StatusCode::BAD_GATEWAY, msg).into_response())
            }
        }
        Ok(Ok(ForwarderProxyReply::Timeout)) | Ok(Err(_)) | Err(_) => {
            Err((StatusCode::GATEWAY_TIMEOUT, "forwarder request timed out").into_response())
        }
    }
}
```

Then one handler per endpoint. Example handlers:

```rust
pub async fn get_reader_info(
    State(state): State<Arc<AppState>>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    match send_reader_control(&state, &forwarder_id, &reader_ip, ReaderControlAction::GetInfo).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => e,
    }
}

pub async fn sync_clock(
    State(state): State<Arc<AppState>>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    match send_reader_control(&state, &forwarder_id, &reader_ip, ReaderControlAction::SyncClock).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => e,
    }
}

#[derive(Deserialize)]
pub struct SetReadModeBody {
    pub mode: String,
    pub timeout: u8,
}

pub async fn set_read_mode(
    State(state): State<Arc<AppState>>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
    Json(body): Json<SetReadModeBody>,
) -> impl IntoResponse {
    match send_reader_control(&state, &forwarder_id, &reader_ip, ReaderControlAction::SetReadMode { mode: body.mode, timeout: body.timeout }).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => e,
    }
}

#[derive(Deserialize)]
pub struct SetTtoBody {
    pub enabled: bool,
}

pub async fn set_tto(
    State(state): State<Arc<AppState>>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
    Json(body): Json<SetTtoBody>,
) -> impl IntoResponse {
    match send_reader_control(&state, &forwarder_id, &reader_ip, ReaderControlAction::SetTto { enabled: body.enabled }).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => e,
    }
}

#[derive(Deserialize)]
pub struct SetRecordingBody {
    pub enabled: bool,
}

pub async fn set_recording(
    State(state): State<Arc<AppState>>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
    Json(body): Json<SetRecordingBody>,
) -> impl IntoResponse {
    match send_reader_control(&state, &forwarder_id, &reader_ip, ReaderControlAction::SetRecording { enabled: body.enabled }).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => e,
    }
}

pub async fn clear_records(
    State(state): State<Arc<AppState>>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    // Fire-and-forget: send the command but don't wait for completion
    let senders = state.forwarder_command_senders.read().await;
    let Some(tx) = senders.get(&forwarder_id).cloned() else {
        return (StatusCode::NOT_FOUND, "forwarder not connected").into_response();
    };
    drop(senders);

    let request_id = Uuid::new_v4().to_string();
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();

    let cmd = ForwarderCommand::ReaderControl {
        request_id,
        reader_ip,
        action: ReaderControlAction::ClearRecords,
        reply: reply_tx,
    };

    if tx.send(cmd).await.is_err() {
        return (StatusCode::BAD_GATEWAY, "forwarder disconnected").into_response();
    }

    StatusCode::ACCEPTED.into_response()
}

pub async fn start_download(
    State(state): State<Arc<AppState>>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    // Fire-and-forget: same pattern as clear_records
    let senders = state.forwarder_command_senders.read().await;
    let Some(tx) = senders.get(&forwarder_id).cloned() else {
        return (StatusCode::NOT_FOUND, "forwarder not connected").into_response();
    };
    drop(senders);

    let request_id = Uuid::new_v4().to_string();
    let (reply_tx, _reply_rx) = tokio::sync::oneshot::channel();

    let cmd = ForwarderCommand::ReaderControl {
        request_id,
        reader_ip,
        action: ReaderControlAction::StartDownload,
        reply: reply_tx,
    };

    if tx.send(cmd).await.is_err() {
        return (StatusCode::BAD_GATEWAY, "forwarder disconnected").into_response();
    }

    StatusCode::ACCEPTED.into_response()
}

pub async fn stop_download(
    State(state): State<Arc<AppState>>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    match send_reader_control(&state, &forwarder_id, &reader_ip, ReaderControlAction::StopDownload).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => e,
    }
}

pub async fn refresh(
    State(state): State<Arc<AppState>>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    match send_reader_control(&state, &forwarder_id, &reader_ip, ReaderControlAction::Refresh).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => e,
    }
}

pub async fn reconnect(
    State(state): State<Arc<AppState>>,
    Path((forwarder_id, reader_ip)): Path<(String, String)>,
) -> impl IntoResponse {
    match send_reader_control(&state, &forwarder_id, &reader_ip, ReaderControlAction::Reconnect).await {
        Ok(resp) => Json(resp).into_response(),
        Err(e) => e,
    }
}

pub async fn get_all_reader_states(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let cache = state.reader_states.read().await;
    let states: Vec<_> = cache.values().cloned().collect();
    Json(states).into_response()
}
```

**Step 2: Register the module in mod.rs**

In `services/server/src/http/mod.rs`, add:

```rust
pub mod reader_control;
```

**Step 3: Register routes in lib.rs**

In `services/server/src/lib.rs` `build_router` function (around lines 97-112, after the forwarder config routes), add:

```rust
.route("/api/v1/reader-states", get(http::reader_control::get_all_reader_states))
.route("/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/info", get(http::reader_control::get_reader_info))
.route("/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/sync-clock", post(http::reader_control::sync_clock))
.route("/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/read-mode", put(http::reader_control::set_read_mode))
.route("/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/tto", put(http::reader_control::set_tto))
.route("/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/recording", put(http::reader_control::set_recording))
.route("/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/clear-records", post(http::reader_control::clear_records))
.route("/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/download-reads", post(http::reader_control::start_download))
.route("/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/stop-download", post(http::reader_control::stop_download))
.route("/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/refresh", post(http::reader_control::refresh))
.route("/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/reconnect", post(http::reader_control::reconnect))
```

Note: Axum uses `{param}` syntax for path parameters. Check the existing routes for the exact syntax used in this project (it may be `:param` if using an older Axum version — match whatever the existing forwarder routes use).

**Step 4: Verify it compiles**

Run: `cargo check -p rt-server`
Expected: compiles

**Step 5: Commit**

```bash
git add services/server/src/http/reader_control.rs services/server/src/http/mod.rs services/server/src/lib.rs
git commit -m "feat(server): add reader control HTTP endpoints"
```

---

### Task 5: Handle ReaderControlRequest on the forwarder side

**Files:**
- Modify: `services/forwarder/src/uplink.rs`
- Modify: `services/forwarder/src/main.rs`

**Step 1: Add ReaderControlRequest to SendBatchResult**

In `services/forwarder/src/uplink.rs`, add a new variant to the `SendBatchResult` enum (around line 53-65):

```rust
    ReaderControl(rt_protocol::ReaderControlRequest),
```

And add the match arm in `send_batch` (around line 234-241):

```rust
WsMessage::ReaderControlRequest(req) => {
    return Ok(SendBatchResult::ReaderControl(req));
}
```

**Step 2: Create the reader control handler function**

In `services/forwarder/src/main.rs`, add a new async function `handle_reader_control_message`. This function:
1. Receives a `ReaderControlRequest`, the `control_clients` map, the `subsystem` (for cached reader info), and the `UplinkSession`
2. Looks up the `ControlClient` by `req.reader_ip`
3. Matches on `req.action` and delegates to the existing `ControlClient` methods
4. Builds and sends back a `WsMessage::ReaderControlResponse`

```rust
async fn handle_reader_control_message(
    session: &mut UplinkSession,
    req: rt_protocol::ReaderControlRequest,
    control_clients: &Arc<RwLock<HashMap<String, Arc<ControlClient>>>>,
    subsystem: &Arc<Mutex<SubsystemStatus>>,
    download_trackers: &Arc<RwLock<HashMap<String, Arc<Mutex<DownloadTracker>>>>>,
    reconnect_notifies: &Arc<RwLock<HashMap<String, Arc<Notify>>>>,
    ui_tx: &broadcast::Sender<ForwarderUiEvent>,
) -> Result<(), UplinkError> {
    let clients = control_clients.read().await;
    let client = clients.get(&req.reader_ip).cloned();
    drop(clients);

    let response = match client {
        None => ReaderControlResponse {
            request_id: req.request_id,
            reader_ip: req.reader_ip,
            success: false,
            error: Some("reader not found".into()),
            reader_info: None,
        },
        Some(client) => {
            execute_reader_action(
                &req, &client, subsystem, download_trackers, reconnect_notifies, ui_tx,
            ).await
        }
    };

    let msg = WsMessage::ReaderControlResponse(response);
    session.send_message(&msg).await?;
    Ok(())
}
```

The `execute_reader_action` function handles each action variant, reusing existing `ControlClient` methods and the `run_status_poll_merge_successes` / `run_status_poll` functions. For fire-and-forget actions (`ClearRecords`, `StartDownload`), spawn a background task and return success immediately.

For converting the forwarder's internal `reader_control::ReaderInfo` to `rt_protocol::ReaderInfo`, add a conversion function (or `impl From<>`) that maps between the two types.

**Step 3: Wire into the main loop**

In `services/forwarder/src/main.rs`, add `SendBatchResult::ReaderControl` match arms in both the replay loop (around line 895-935) and the uplink loop (around line 1087-1115):

```rust
Ok(SendBatchResult::ReaderControl(req)) => {
    if let Err(e) = handle_reader_control_message(
        &mut session,
        req,
        &control_clients,
        &subsystem,
        &download_trackers,
        &reconnect_notifies,
        &ui_tx,
    ).await {
        warn!(error = %e, "reader control handler failed");
        break 'uplink;
    }
}
```

**Step 4: Verify it compiles**

Run: `cargo check -p rt-forwarder`
Expected: compiles

**Step 5: Commit**

```bash
git add services/forwarder/src/uplink.rs services/forwarder/src/main.rs
git commit -m "feat(forwarder): handle reader control requests from server"
```

---

### Task 6: Add unsolicited ReaderInfoUpdate pushes from forwarder

**Files:**
- Modify: `services/forwarder/src/main.rs`

**Step 1: Identify where ForwarderUiEvent::ReaderUpdated and ReaderInfoUpdated are emitted**

These are the points where reader state changes. At each of these points, also send a `WsMessage::ReaderInfoUpdate` upstream if the uplink session is active. The simplest approach: subscribe to the existing `ui_tx` broadcast channel in the uplink task and forward relevant events as WS messages.

**Step 2: Add a ui_rx subscriber in the uplink loop**

In the uplink task, before the main `'uplink` loop, subscribe to `ui_tx`:

```rust
let mut ui_rx = ui_tx.subscribe();
```

Then add a new `tokio::select!` arm (or integrate into the existing batch-sending flow) that drains `ui_rx` and sends `ReaderInfoUpdate` / `ReaderDownloadProgress` messages upstream:

```rust
// In a select! or between batch sends:
while let Ok(event) = ui_rx.try_recv() {
    match event {
        ForwarderUiEvent::ReaderUpdated { ip, state, reader_info } => {
            let update = WsMessage::ReaderInfoUpdate(rt_protocol::ReaderInfoUpdate {
                reader_ip: ip,
                state: state.to_string(),
                reader_info: reader_info.map(convert_reader_info),
            });
            session.send_message(&update).await?;
        }
        ForwarderUiEvent::ReaderInfoUpdated { ip, info } => {
            // Get current state from subsystem
            let sub = subsystem.lock().await;
            let state_str = sub.readers.get(&ip)
                .map(|r| r.state.to_string())
                .unwrap_or_else(|| "unknown".into());
            drop(sub);
            let update = WsMessage::ReaderInfoUpdate(rt_protocol::ReaderInfoUpdate {
                reader_ip: ip,
                state: state_str,
                reader_info: Some(convert_reader_info(info)),
            });
            session.send_message(&update).await?;
        }
        _ => {} // ignore other UI events
    }
}
```

The exact integration point depends on the forwarder's uplink loop structure — it may need to be a `select!` branch alongside the batch send, or drained at the top of each loop iteration.

**Step 3: Add download progress forwarding**

Similarly, when a download is active, subscribe to the `DownloadTracker`'s broadcast and forward progress as `WsMessage::ReaderDownloadProgress`. This can be done inside the `handle_reader_control_message` for `StartDownload` — spawn a task that subscribes and forwards.

**Step 4: Verify it compiles**

Run: `cargo check -p rt-forwarder`
Expected: compiles

**Step 5: Commit**

```bash
git add services/forwarder/src/main.rs
git commit -m "feat(forwarder): push reader state changes upstream via WS"
```

---

### Task 7: Move shared view-model helpers to shared-ui

**Files:**
- Create: `apps/shared-ui/src/lib/reader-view-model.ts`
- Create: `apps/shared-ui/src/lib/read-mode-form.ts`
- Modify: `apps/shared-ui/src/lib/index.ts`
- Modify: `apps/forwarder-ui/src/lib/status-view-model.ts`
- Modify: `apps/forwarder-ui/src/lib/read-mode-form.ts`

**Step 1: Create shared reader-view-model.ts**

Copy the following pure functions from `apps/forwarder-ui/src/lib/status-view-model.ts` to `apps/shared-ui/src/lib/reader-view-model.ts`:

- `formatReadMode`
- `formatTtoState`
- `formatClockDrift`
- `readerControlDisabled` (make the state parameter a plain `string` instead of `ReaderStatus["state"]`)
- `computeDownloadPercent` (make the download parameter use an inline type: `{ state: string; reads_received: number; progress: number; total: number } | null | undefined`)
- `computeTickingLastSeen`
- `computeElapsedSecondsSince`

These are all pure functions with no forwarder-specific imports. Adjust the type signatures to use plain types instead of forwarder-specific imports.

**Step 2: Create shared read-mode-form.ts**

Copy the entire `apps/forwarder-ui/src/lib/read-mode-form.ts` to `apps/shared-ui/src/lib/read-mode-form.ts`. Replace the `Config3Info` import with an inline type `{ mode: string; timeout: number }`.

**Step 3: Export from shared-ui index.ts**

In `apps/shared-ui/src/lib/index.ts`, add:

```typescript
export * from "../lib/reader-view-model";
export * from "../lib/read-mode-form";
```

**Step 4: Update forwarder-ui to import from shared-ui**

In `apps/forwarder-ui/src/lib/status-view-model.ts`, remove the moved functions and re-export them from shared-ui:

```typescript
export { formatReadMode, formatTtoState, formatClockDrift, readerControlDisabled, computeDownloadPercent, computeTickingLastSeen, computeElapsedSecondsSince } from "@rusty-timer/shared-ui/lib/reader-view-model";
```

Do the same for `read-mode-form.ts`.

**Step 5: Verify forwarder-ui still builds**

Run: `cd apps/forwarder-ui && npm run build` (or the project's build command)
Expected: builds successfully

**Step 6: Commit**

```bash
git add apps/shared-ui/src/lib/reader-view-model.ts apps/shared-ui/src/lib/read-mode-form.ts apps/shared-ui/src/lib/index.ts apps/forwarder-ui/src/lib/status-view-model.ts apps/forwarder-ui/src/lib/read-mode-form.ts
git commit -m "refactor(shared-ui): move reader view-model helpers to shared package"
```

---

### Task 8: Add reader control API functions and types to server-ui

**Files:**
- Modify: `apps/server-ui/src/lib/api.ts`
- Modify: `apps/server-ui/src/lib/stores.ts`
- Modify: `apps/server-ui/src/lib/sse.ts`

**Step 1: Add TypeScript types for reader state**

In `apps/server-ui/src/lib/api.ts`, add:

```typescript
export interface HardwareInfo {
  fw_version: string | null;
  hw_code: string | null;
  reader_id: string | null;
}

export interface Config3Info {
  mode: string;
  timeout: number;
}

export interface ClockInfo {
  reader_clock: string;
  drift_ms: number;
}

export interface ReaderInfo {
  banner?: string | null;
  hardware?: HardwareInfo | null;
  config?: Config3Info | null;
  tto_enabled?: boolean | null;
  clock?: ClockInfo | null;
  estimated_stored_reads?: number | null;
  recording?: boolean | null;
  connect_failures: number;
}

export interface CachedReaderState {
  forwarder_id: string;
  reader_ip: string;
  state: string;
  reader_info: ReaderInfo | null;
}

export interface ReaderControlResponse {
  request_id: string;
  reader_ip: string;
  success: boolean;
  error?: string | null;
  reader_info?: ReaderInfo | null;
}

export interface ReaderDownloadProgressEvent {
  forwarder_id: string;
  reader_ip: string;
  state: string;
  reads_received: number;
  progress: number;
  total: number;
  error?: string | null;
}
```

**Step 2: Add API functions**

```typescript
export async function getReaderStates(): Promise<CachedReaderState[]> {
  return apiFetch<CachedReaderState[]>(`${BASE}/api/v1/reader-states`);
}

export async function getReaderInfo(forwarderId: string, readerIp: string): Promise<ReaderControlResponse> {
  return apiFetch<ReaderControlResponse>(`${BASE}/api/v1/forwarders/${encodeURIComponent(forwarderId)}/readers/${encodeURIComponent(readerIp)}/info`);
}

export async function syncReaderClock(forwarderId: string, readerIp: string): Promise<ReaderControlResponse> {
  return apiFetch<ReaderControlResponse>(`${BASE}/api/v1/forwarders/${encodeURIComponent(forwarderId)}/readers/${encodeURIComponent(readerIp)}/sync-clock`, { method: "POST" });
}

export async function setReaderReadMode(forwarderId: string, readerIp: string, mode: string, timeout: number): Promise<ReaderControlResponse> {
  return apiFetch<ReaderControlResponse>(`${BASE}/api/v1/forwarders/${encodeURIComponent(forwarderId)}/readers/${encodeURIComponent(readerIp)}/read-mode`, {
    method: "PUT",
    body: JSON.stringify({ mode, timeout }),
  });
}

export async function setReaderTto(forwarderId: string, readerIp: string, enabled: boolean): Promise<ReaderControlResponse> {
  return apiFetch<ReaderControlResponse>(`${BASE}/api/v1/forwarders/${encodeURIComponent(forwarderId)}/readers/${encodeURIComponent(readerIp)}/tto`, {
    method: "PUT",
    body: JSON.stringify({ enabled }),
  });
}

export async function setReaderRecording(forwarderId: string, readerIp: string, enabled: boolean): Promise<ReaderControlResponse> {
  return apiFetch<ReaderControlResponse>(`${BASE}/api/v1/forwarders/${encodeURIComponent(forwarderId)}/readers/${encodeURIComponent(readerIp)}/recording`, {
    method: "PUT",
    body: JSON.stringify({ enabled }),
  });
}

export async function clearReaderRecords(forwarderId: string, readerIp: string): Promise<void> {
  await apiFetch(`${BASE}/api/v1/forwarders/${encodeURIComponent(forwarderId)}/readers/${encodeURIComponent(readerIp)}/clear-records`, { method: "POST" });
}

export async function startReaderDownload(forwarderId: string, readerIp: string): Promise<void> {
  await apiFetch(`${BASE}/api/v1/forwarders/${encodeURIComponent(forwarderId)}/readers/${encodeURIComponent(readerIp)}/download-reads`, { method: "POST" });
}

export async function stopReaderDownload(forwarderId: string, readerIp: string): Promise<void> {
  await apiFetch(`${BASE}/api/v1/forwarders/${encodeURIComponent(forwarderId)}/readers/${encodeURIComponent(readerIp)}/stop-download`, { method: "POST" });
}

export async function refreshReader(forwarderId: string, readerIp: string): Promise<ReaderControlResponse> {
  return apiFetch<ReaderControlResponse>(`${BASE}/api/v1/forwarders/${encodeURIComponent(forwarderId)}/readers/${encodeURIComponent(readerIp)}/refresh`, { method: "POST" });
}

export async function reconnectReader(forwarderId: string, readerIp: string): Promise<ReaderControlResponse> {
  return apiFetch<ReaderControlResponse>(`${BASE}/api/v1/forwarders/${encodeURIComponent(forwarderId)}/readers/${encodeURIComponent(readerIp)}/reconnect`, { method: "POST" });
}
```

**Step 3: Add stores**

In `apps/server-ui/src/lib/stores.ts`, add:

```typescript
import type { CachedReaderState, ReaderDownloadProgressEvent } from "./api";

export const readerStatesStore = writable<Record<string, CachedReaderState>>({});
export const downloadProgressStore = writable<Record<string, ReaderDownloadProgressEvent>>({});

export function setReaderState(key: string, state: CachedReaderState) {
  readerStatesStore.update(s => ({ ...s, [key]: state }));
}

export function removeReaderStatesForForwarder(forwarderId: string) {
  readerStatesStore.update(s => {
    const next: Record<string, CachedReaderState> = {};
    for (const [k, v] of Object.entries(s)) {
      if (v.forwarder_id !== forwarderId) next[k] = v;
    }
    return next;
  });
}

export function setDownloadProgress(key: string, progress: ReaderDownloadProgressEvent) {
  downloadProgressStore.update(s => ({ ...s, [key]: progress }));
}
```

Add `readerStatesStore` and `downloadProgressStore` resets to the existing `resetStores()` function.

**Step 4: Add SSE event handlers**

In `apps/server-ui/src/lib/sse.ts`, add event listeners in `initSSE`:

```typescript
eventSource.addEventListener("reader_info_updated", (e: MessageEvent) => {
  const data = JSON.parse(e.data);
  const key = `${data.forwarder_id}:${data.reader_ip}`;
  setReaderState(key, {
    forwarder_id: data.forwarder_id,
    reader_ip: data.reader_ip,
    state: data.state,
    reader_info: data.reader_info,
  });
});

eventSource.addEventListener("reader_download_progress", (e: MessageEvent) => {
  const data = JSON.parse(e.data);
  const key = `${data.forwarder_id}:${data.reader_ip}`;
  setDownloadProgress(key, data);
});
```

In the `resync()` function, add `getReaderStates()` to the `Promise.allSettled` call and populate `readerStatesStore` from the results.

**Step 5: Verify it builds**

Run: `cd apps/server-ui && npm run check` (or the project's typecheck command)
Expected: no type errors

**Step 6: Commit**

```bash
git add apps/server-ui/src/lib/api.ts apps/server-ui/src/lib/stores.ts apps/server-ui/src/lib/sse.ts
git commit -m "feat(server-ui): add reader control API, stores, and SSE handlers"
```

---

### Task 9: Add expandable reader detail panel to dashboard stream cards

**Files:**
- Modify: `apps/server-ui/src/routes/+page.svelte`

**Step 1: Add reader control state variables**

At the top of the `<script>` block, add:

```typescript
import { readerStatesStore, downloadProgressStore } from "$lib/stores";
import { formatReadMode, formatTtoState, formatClockDrift, readerControlDisabled, computeDownloadPercent, computeTickingLastSeen } from "@rusty-timer/shared-ui/lib/reader-view-model";
import { READ_MODE_OPTIONS, shouldShowTimeoutInput, initialTimeoutDraft, resolveTimeoutSeconds } from "@rusty-timer/shared-ui/lib/read-mode-form";
import { syncReaderClock, setReaderReadMode, setReaderTto, setReaderRecording, clearReaderRecords, startReaderDownload, stopReaderDownload, refreshReader, reconnectReader } from "$lib/api";

let expandedReader = $state<string | null>(null);
let controlBusy: Record<string, boolean> = $state({});
let controlFeedback: Record<string, { kind: "ok" | "err"; message: string } | undefined> = $state({});
let readModeDrafts: Record<string, string> = $state({});
let readModeTimeoutDrafts: Record<string, string> = $state({});
let clockTickNow = $state(Date.now());

// Tick every second for ticking displays
$effect(() => {
  const interval = setInterval(() => { clockTickNow = Date.now(); }, 1000);
  return () => clearInterval(interval);
});

function toggleReaderExpand(key: string) {
  expandedReader = expandedReader === key ? null : key;
}

function readerKey(forwarderId: string, readerIp: string): string {
  return `${forwarderId}:${readerIp}`;
}
```

**Step 2: Add the expand/collapse button to the stream card header**

In the stream card (around line 337-341, in the header area), add a "Details" button:

```svelte
<button
  onclick={() => toggleReaderExpand(readerKey(stream.forwarder_id, stream.reader_ip))}
  class="ml-auto text-xs text-muted hover:text-foreground transition-colors flex items-center gap-1"
  aria-expanded={expandedReader === readerKey(stream.forwarder_id, stream.reader_ip)}
>
  Details
  <span class="inline-block transition-transform {expandedReader === readerKey(stream.forwarder_id, stream.reader_ip) ? 'rotate-180' : ''}">▾</span>
</button>
```

**Step 3: Add the expandable reader detail panel**

After the existing stream card content (after the footer row, around line 403), add the expandable panel. This should closely mirror the forwarder-ui's expanded detail panel layout:

```svelte
{#if expandedReader === readerKey(stream.forwarder_id, stream.reader_ip)}
  {@const key = readerKey(stream.forwarder_id, stream.reader_ip)}
  {@const rs = $readerStatesStore[key]}
  {@const info = rs?.reader_info}
  {@const busy = controlBusy[key]}
  {@const disabled = !stream.online || readerControlDisabled(rs?.state ?? "disconnected", busy)}
  {@const dp = $downloadProgressStore[key]}

  <div class="mt-4 pt-4 border-t border-border">
    {#if !rs}
      <p class="text-sm text-muted">No reader data available</p>
    {:else}
      <!-- Info grid -->
      <div class="grid grid-cols-2 gap-x-8 gap-y-2 text-sm mb-4">
        {#if info?.banner}
          <div class="col-span-2">
            <span class="text-muted">Banner</span>
            <span class="ml-2 font-mono text-xs">{info.banner}</span>
          </div>
        {/if}
        <div><span class="text-muted">Firmware</span> <span>{info?.hardware?.fw_version ?? "—"}</span></div>
        <div><span class="text-muted">Hardware</span> <span>{info?.hardware?.hw_code ?? "—"}</span></div>
        <div><span class="text-muted">Clock Drift</span> <span>{formatClockDrift(info?.clock?.drift_ms)}</span></div>
        <div><span class="text-muted">Reader State</span> <StatusBadge label={rs.state} state={rs.state === "connected" ? "ok" : rs.state === "connecting" ? "warn" : "err"} /></div>
        <div><span class="text-muted">Read Mode</span> <span>{formatReadMode(info?.config?.mode)}</span></div>
        <div><span class="text-muted">TTO</span> <span>{formatTtoState(info?.tto_enabled)}</span></div>
        <div><span class="text-muted">Recording</span> <span>{info?.recording != null ? (info.recording ? "On" : "Off") : "—"}</span></div>
        <div><span class="text-muted">Stored Reads</span> <span>{info?.estimated_stored_reads ?? "—"}</span></div>
      </div>

      <!-- Read mode controls -->
      <div class="flex items-center gap-2 text-sm mb-3">
        <select
          class="bg-surface-2 border border-border rounded px-2 py-1 text-sm"
          value={readModeDrafts[key] ?? info?.config?.mode ?? "raw"}
          onchange={(e) => { readModeDrafts[key] = e.currentTarget.value; }}
          {disabled}
        >
          {#each READ_MODE_OPTIONS as opt}
            <option value={opt.value}>{opt.label}</option>
          {/each}
        </select>
        {#if shouldShowTimeoutInput(readModeDrafts[key] ?? info?.config?.mode)}
          <input
            type="number"
            class="bg-surface-2 border border-border rounded px-2 py-1 text-sm w-16"
            value={readModeTimeoutDrafts[key] ?? initialTimeoutDraft(info?.config?.timeout)}
            onchange={(e) => { readModeTimeoutDrafts[key] = e.currentTarget.value; }}
            {disabled}
            min="1" max="255"
          />
          <span class="text-muted text-xs">sec</span>
        {/if}
        <button
          class="btn btn-sm btnPrimary"
          {disabled}
          onclick={() => handleSetReadMode(stream.forwarder_id, stream.reader_ip, key)}
        >Apply</button>
      </div>

      <!-- TTO toggle -->
      <div class="flex items-center gap-2 text-sm mb-3">
        <button
          class="btn btn-sm {info?.tto_enabled ? 'bg-red-600 hover:bg-red-700' : 'btnPrimary'} text-white"
          {disabled}
          onclick={() => handleSetTto(stream.forwarder_id, stream.reader_ip, key, !(info?.tto_enabled ?? false))}
        >{info?.tto_enabled ? "Disable TTO" : "Enable TTO"}</button>
      </div>

      <!-- Action buttons -->
      <div class="flex items-center gap-3 pt-3 border-t border-border flex-wrap">
        <button class="btn btn-sm btnPrimary" {disabled} onclick={() => handleSyncClock(stream.forwarder_id, stream.reader_ip, key)}>Sync Clock</button>
        <button class="btn btn-sm" {disabled} onclick={() => handleRefresh(stream.forwarder_id, stream.reader_ip, key)}>Refresh</button>
        <button
          class="btn btn-sm {info?.recording ? 'bg-red-600 hover:bg-red-700 text-white' : 'bg-green-600 hover:bg-green-700 text-white'}"
          {disabled}
          onclick={() => handleSetRecording(stream.forwarder_id, stream.reader_ip, key, !(info?.recording ?? false))}
        >{info?.recording ? "Stop Recording" : "Start Recording"}</button>
        <button class="btn btn-sm btnPrimary" {disabled} onclick={() => handleStartDownload(stream.forwarder_id, stream.reader_ip, key)}>Download Reads</button>
        <button class="btn btn-sm bg-red-600 hover:bg-red-700 text-white" {disabled} onclick={() => handleClearRecords(stream.forwarder_id, stream.reader_ip, key)}>Clear Records</button>
        <button class="btn btn-sm" {disabled} onclick={() => handleReconnect(stream.forwarder_id, stream.reader_ip, key)}>Reconnect</button>
      </div>

      <!-- Download progress -->
      {#if dp?.state === "downloading"}
        {@const percent = computeDownloadPercent(dp, info?.estimated_stored_reads)}
        <div class="mt-3">
          <div class="flex justify-between text-xs text-muted mb-1">
            <span>Downloading...</span>
            <span>{percent}%</span>
          </div>
          <div class="h-2 rounded-full bg-surface-2">
            <div class="h-full rounded-full bg-accent transition-all" style="width: {percent}%"></div>
          </div>
        </div>
      {/if}

      <!-- Feedback banner -->
      {#if controlFeedback[key]}
        <div class="mt-3">
          <AlertBanner
            variant={controlFeedback[key]?.kind === "ok" ? "ok" : "err"}
            message={controlFeedback[key]?.message ?? ""}
            onDismiss={() => { controlFeedback[key] = undefined; }}
          />
        </div>
      {/if}
    {/if}
  </div>
{/if}
```

**Step 4: Add handler functions**

Add these handler functions in the `<script>` block. Each follows the same pattern: set busy, call API, update feedback, clear busy.

```typescript
async function handleSyncClock(forwarderId: string, readerIp: string, key: string) {
  controlBusy[key] = true;
  controlFeedback[key] = undefined;
  try {
    const resp = await syncReaderClock(forwarderId, readerIp);
    controlFeedback[key] = { kind: "ok", message: `Clock synced — drift: ${formatClockDrift(resp.reader_info?.clock?.drift_ms)}` };
  } catch (e: any) {
    controlFeedback[key] = { kind: "err", message: e.message ?? "Failed to sync clock" };
  }
  controlBusy[key] = false;
}

async function handleSetReadMode(forwarderId: string, readerIp: string, key: string) {
  controlBusy[key] = true;
  controlFeedback[key] = undefined;
  try {
    const mode = readModeDrafts[key] ?? "raw";
    const timeout = resolveTimeoutSeconds(readModeTimeoutDrafts[key] ?? "5", null);
    await setReaderReadMode(forwarderId, readerIp, mode, timeout);
    controlFeedback[key] = { kind: "ok", message: `Read mode set to ${formatReadMode(mode)}` };
  } catch (e: any) {
    controlFeedback[key] = { kind: "err", message: e.message ?? "Failed to set read mode" };
  }
  controlBusy[key] = false;
}

async function handleSetTto(forwarderId: string, readerIp: string, key: string, enabled: boolean) {
  controlBusy[key] = true;
  controlFeedback[key] = undefined;
  try {
    await setReaderTto(forwarderId, readerIp, enabled);
    controlFeedback[key] = { kind: "ok", message: `TTO ${enabled ? "enabled" : "disabled"}` };
  } catch (e: any) {
    controlFeedback[key] = { kind: "err", message: e.message ?? "Failed to set TTO" };
  }
  controlBusy[key] = false;
}

async function handleSetRecording(forwarderId: string, readerIp: string, key: string, enabled: boolean) {
  controlBusy[key] = true;
  controlFeedback[key] = undefined;
  try {
    await setReaderRecording(forwarderId, readerIp, enabled);
    controlFeedback[key] = { kind: "ok", message: `Recording ${enabled ? "started" : "stopped"}` };
  } catch (e: any) {
    controlFeedback[key] = { kind: "err", message: e.message ?? "Failed to set recording" };
  }
  controlBusy[key] = false;
}

async function handleRefresh(forwarderId: string, readerIp: string, key: string) {
  controlBusy[key] = true;
  controlFeedback[key] = undefined;
  try {
    await refreshReader(forwarderId, readerIp);
    controlFeedback[key] = { kind: "ok", message: "Reader refreshed" };
  } catch (e: any) {
    controlFeedback[key] = { kind: "err", message: e.message ?? "Failed to refresh" };
  }
  controlBusy[key] = false;
}

async function handleClearRecords(forwarderId: string, readerIp: string, key: string) {
  controlBusy[key] = true;
  controlFeedback[key] = undefined;
  try {
    await clearReaderRecords(forwarderId, readerIp);
    controlFeedback[key] = { kind: "ok", message: "Clear records initiated" };
  } catch (e: any) {
    controlFeedback[key] = { kind: "err", message: e.message ?? "Failed to clear records" };
  }
  controlBusy[key] = false;
}

async function handleStartDownload(forwarderId: string, readerIp: string, key: string) {
  controlBusy[key] = true;
  controlFeedback[key] = undefined;
  try {
    await startReaderDownload(forwarderId, readerIp);
    controlFeedback[key] = { kind: "ok", message: "Download started" };
  } catch (e: any) {
    controlFeedback[key] = { kind: "err", message: e.message ?? "Failed to start download" };
  }
  controlBusy[key] = false;
}

async function handleReconnect(forwarderId: string, readerIp: string, key: string) {
  controlBusy[key] = true;
  controlFeedback[key] = undefined;
  try {
    await reconnectReader(forwarderId, readerIp);
    controlFeedback[key] = { kind: "ok", message: "Reconnect initiated" };
  } catch (e: any) {
    controlFeedback[key] = { kind: "err", message: e.message ?? "Failed to reconnect" };
  }
  controlBusy[key] = false;
}
```

**Step 5: Verify it builds**

Run: `cd apps/server-ui && npm run build`
Expected: builds successfully

**Step 6: Commit**

```bash
git add apps/server-ui/src/routes/+page.svelte
git commit -m "feat(server-ui): add expandable reader control panel to stream cards"
```

---

### Task 10: Integration testing and verification

**Files:**
- Test: existing integration test infrastructure

**Step 1: Run all Rust tests**

Run: `cargo test --workspace`
Expected: all existing tests pass (new code doesn't break anything)

**Step 2: Run server-ui build**

Run: `cd apps/server-ui && npm run build && npm run check`
Expected: builds and type-checks cleanly

**Step 3: Run forwarder-ui build**

Run: `cd apps/forwarder-ui && npm run build && npm run check`
Expected: builds and type-checks cleanly (shared-ui refactor didn't break anything)

**Step 4: Manual smoke test**

If a test environment is available:
1. Start the server
2. Connect a forwarder with a reader
3. Open the dashboard, verify stream cards show "Details" button
4. Expand a stream card, verify reader info populates
5. Test each control button (sync clock, read mode, TTO, recording, refresh, reconnect)
6. Verify SSE pushes update the UI in real-time

**Step 5: Commit any fixes**

If any fixes were needed during testing, commit them:

```bash
git add -u
git commit -m "fix: address integration test findings"
```
