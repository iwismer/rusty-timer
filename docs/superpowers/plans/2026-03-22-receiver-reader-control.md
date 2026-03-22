# Receiver Reader Control Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add reader control (clock sync, read mode, TTO, recording, download, etc.) to the receiver via WS proxy, tunnel reader info/download progress events to receivers, and add reader control UI to both the server dashboard stream detail page and the receiver streams tab.

**Architecture:** A single `ReceiverProxyReaderControlRequest/Response` WS message pair wraps the existing `ReaderControlAction` enum. The server proxies these to forwarders using the existing `ForwarderCommand::ReaderControl` channel. `ReaderInfoUpdate` and `ReaderDownloadProgress` are tunneled to receivers via the sentinel-in-broadcast-channel pattern already used by `ReaderStatusChanged`. A shared `ReaderControlPanel.svelte` component is used by both UIs.

**Tech Stack:** Rust (rt-protocol, Axum server, Tauri receiver), Svelte 5, Tailwind CSS v4, TypeScript

**Spec:** `docs/superpowers/specs/2026-03-22-receiver-reader-control-design.md`

---

## Dependency Graph

```
Task 1 (protocol) ──┬──> Task 2 (server proxy handler)
                     │        │
                     │        └──> Task 4 (receiver backend) ──> Task 7 (receiver UI)
                     │
                     ├──> Task 3 (server sentinel tunneling)
                     │        │
                     │        └──> Task 4 (receiver backend)
                     │
                     ├──> Task 5 (shared UI component) ──┬──> Task 6 (server UI)
                     │                                    └──> Task 7 (receiver UI)
                     │
                     └──> Task 6 (server UI) [also depends on Task 5]
```

**Parallelizable after Task 1:** Tasks 2, 3, and 5 can run in parallel. Task 4 depends on 2+3. Tasks 6 and 7 depend on 5. Task 7 additionally depends on 4.

**Merge conflict note:** Tasks 2 and 3 both modify `services/server/src/ws_receiver.rs` (proxy handler and sentinel intercepts respectively). If run by parallel subagents, the second to merge will need to resolve conflicts. Consider running Task 3's `ws_receiver.rs` changes after Task 2, or have the later agent rebase.

---

## File Map

| File | Action | Task | Responsibility |
|------|--------|------|----------------|
| `crates/rt-protocol/src/lib.rs` | Modify | 1 | New structs, WsMessage variants, sentinel constants |
| `services/server/src/ws_receiver.rs` | Modify | 2 | Proxy handler + sentinel intercepts (Task 3) |
| `services/server/src/ws_forwarder.rs` | Modify | 3 | Sentinel broadcasts for ReaderInfoUpdate/DownloadProgress |
| `services/receiver/src/session.rs` | Modify | 4 | WsCommand::new() + incoming message handling |
| `services/receiver/src/ui_events.rs` | Modify | 4 | New ReceiverUiEvent variants |
| `services/receiver/src/control_api.rs` | Modify | 4 | New reader control proxy functions |
| `apps/receiver-ui/src-tauri/src/main.rs` | Modify | 4 | New Tauri command handlers + event bridge |
| `apps/shared-ui/src/components/ReaderControlPanel.svelte` | Create | 5 | Shared reader control UI component |
| `apps/shared-ui/src/lib/index.ts` | Modify | 5 | Export new component |
| `apps/server-ui/src/routes/streams/[streamId]/+page.svelte` | Modify | 6 | Reader control card on stream detail page |
| `apps/receiver-ui/src/lib/api.ts` | Modify | 7 | New Tauri invoke functions |
| `apps/receiver-ui/src/lib/sse.ts` | Modify | 7 | New event listeners |
| `apps/receiver-ui/src/lib/store.svelte.ts` | Modify | 7 | Store handlers for reader info/progress events |
| `apps/receiver-ui/src/lib/components/StreamsTab.svelte` | Modify | 7 | Reader control in expanded row |

---

## Task 1: Protocol Layer — New Types and WsMessage Variants

**Files:**
- Modify: `crates/rt-protocol/src/lib.rs` (add after line ~838 for structs, after line ~908 for enum variants, after line ~127 for constants, tests after line ~1665)

### Steps

- [ ] **Step 1: Add sentinel constants**

In `crates/rt-protocol/src/lib.rs`, after the existing constants at line 127 (`READER_STATUS_CHANGED_READ_TYPE`), add:

```rust
pub const READER_INFO_UPDATED_READ_TYPE: &str = "__reader_info_updated";
pub const READER_DOWNLOAD_PROGRESS_READ_TYPE: &str = "__reader_download_progress";
```

- [ ] **Step 2: Add the 4 new structs**

After the last `ReceiverProxy*` struct pair (around line 838, after `ReceiverProxyFileUploadResponse`), add:

```rust
/// Receiver-to-server: request a reader control action proxied to a forwarder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyReaderControlRequest {
    pub request_id: String,
    pub forwarder_id: String,
    pub reader_ip: String,
    pub action: ReaderControlAction,
}

/// Server-to-receiver: result of a proxied reader control action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverProxyReaderControlResponse {
    pub request_id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
}

/// Server-to-receiver: broadcast reader info update (tunneled via sentinel).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverReaderInfoUpdate {
    pub stream_id: Uuid,
    pub reader_ip: String,
    pub state: ReaderConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
}

/// Server-to-receiver: broadcast download progress (tunneled via sentinel).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverReaderDownloadProgress {
    pub stream_id: Uuid,
    pub reader_ip: String,
    pub state: DownloadState,
    pub reads_received: u32,
    pub progress: u64,
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
```

- [ ] **Step 3: Add 4 new WsMessage variants**

In the `WsMessage` enum (after the last variant around line 908), add:

```rust
ReceiverProxyReaderControlRequest(ReceiverProxyReaderControlRequest),
ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse),
ReceiverReaderInfoUpdate(ReceiverReaderInfoUpdate),
ReceiverReaderDownloadProgress(ReceiverReaderDownloadProgress),
```

- [ ] **Step 4: Write serde round-trip tests**

In the `#[cfg(test)] mod tests` block (after the last test around line 1665), add round-trip tests following the existing pattern (see `reader_control_request_round_trip` at line 1008 for reference):

```rust
#[test]
fn receiver_proxy_reader_control_request_round_trip() {
    let msg = WsMessage::ReceiverProxyReaderControlRequest(ReceiverProxyReaderControlRequest {
        request_id: "req-1".to_owned(),
        forwarder_id: "fwd-1".to_owned(),
        reader_ip: "192.168.1.100:10000".to_owned(),
        action: ReaderControlAction::SyncClock,
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
    assert!(json.contains("\"kind\":\"receiver_proxy_reader_control_request\""));
}

#[test]
fn receiver_proxy_reader_control_response_round_trip() {
    let msg = WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
        request_id: "req-1".to_owned(),
        ok: true,
        error: None,
        reader_info: None,
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
    assert!(json.contains("\"kind\":\"receiver_proxy_reader_control_response\""));
}

#[test]
fn receiver_reader_info_update_round_trip() {
    let msg = WsMessage::ReceiverReaderInfoUpdate(ReceiverReaderInfoUpdate {
        stream_id: Uuid::nil(),
        reader_ip: "192.168.1.100:10000".to_owned(),
        state: ReaderConnectionState::Connected,
        reader_info: None,
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
    assert!(json.contains("\"kind\":\"receiver_reader_info_update\""));
}

#[test]
fn receiver_reader_download_progress_round_trip() {
    let msg = WsMessage::ReceiverReaderDownloadProgress(ReceiverReaderDownloadProgress {
        stream_id: Uuid::nil(),
        reader_ip: "192.168.1.100:10000".to_owned(),
        state: DownloadState::Downloading,
        reads_received: 42,
        progress: 100,
        total: 1000,
        error: None,
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
    assert!(json.contains("\"kind\":\"receiver_reader_download_progress\""));
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rt-protocol`
Expected: All tests pass including the 4 new round-trip tests.

- [ ] **Step 6: Commit**

```bash
git add crates/rt-protocol/src/lib.rs
git commit -m "feat(rt-protocol): add receiver reader control proxy and tunnel types"
```

---

## Task 2: Server — WS Receiver Proxy Handler

**Files:**
- Modify: `services/server/src/ws_receiver.rs` (new proxy handler function + match arm)

**Depends on:** Task 1

### Context

The existing proxy pattern is:
1. An `async fn proxy_*_reply(state, req) -> WsMessage` function
2. A match arm in the session loop that spawns it on `pending_proxy_replies: JoinSet<WsMessage>`
3. The JoinSet result is polled and sent back over the WS

For reader control, we need to replicate the logic from `services/server/src/http/reader_control.rs` — specifically `send_reader_control` (lines 42-100) and `send_fire_and_forget` (lines 124-160) — but return a `WsMessage` instead of an HTTP response.

### Steps

- [ ] **Step 1: Add the `proxy_reader_control_reply` async function**

In `services/server/src/ws_receiver.rs`, near the other `proxy_*_reply` functions (around line 152-174), add:

```rust
async fn proxy_reader_control_reply(
    state: AppState,
    req: rt_protocol::ReceiverProxyReaderControlRequest,
) -> WsMessage {
    use rt_protocol::{ReceiverProxyReaderControlResponse, ReaderControlAction};

    // Validate reader_ip format (consistent with HTTP endpoints)
    if req.reader_ip.parse::<std::net::SocketAddrV4>().is_err() {
        return WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
            request_id: req.request_id,
            ok: false,
            error: Some("invalid reader_ip format".to_owned()),
            reader_info: None,
        });
    }

    // Look up the forwarder's command sender
    let tx = {
        let senders = state.forwarder_command_senders.read().await;
        senders.get(&req.forwarder_id).cloned()
    };
    let Some(tx) = tx else {
        return WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
            request_id: req.request_id,
            ok: false,
            error: Some("forwarder not connected".to_owned()),
            reader_info: None,
        });
    };

    // Fire-and-forget for ClearRecords and StartDownload
    let is_fire_and_forget = matches!(
        req.action,
        ReaderControlAction::ClearRecords | ReaderControlAction::StartDownload
    );

    if is_fire_and_forget {
        let cmd = crate::state::ForwarderCommand::ReaderControlFireAndForget {
            reader_ip: req.reader_ip.clone(),
            action: req.action,
        };
        return match tx.try_send(cmd) {
            Ok(()) => WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
                request_id: req.request_id,
                ok: true,
                error: None,
                reader_info: None,
            }),
            Err(_) => WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
                request_id: req.request_id,
                ok: false,
                error: Some("forwarder command queue full or closed".to_owned()),
                reader_info: None,
            }),
        };
    }

    // Request/response path
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let fwd_request_id = uuid::Uuid::new_v4().to_string();
    let cmd = crate::state::ForwarderCommand::ReaderControl {
        request_id: fwd_request_id,
        reader_ip: req.reader_ip.clone(),
        action: req.action,
        reply: reply_tx,
    };

    let timeout = std::time::Duration::from_secs(10);
    match tokio::time::timeout(timeout, tx.send(cmd)).await {
        Ok(Ok(())) => {} // sent successfully
        Ok(Err(_)) => {
            return WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
                request_id: req.request_id,
                ok: false,
                error: Some("forwarder disconnected".to_owned()),
                reader_info: None,
            });
        }
        Err(_) => {
            return WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
                request_id: req.request_id,
                ok: false,
                error: Some("timeout sending to forwarder".to_owned()),
                reader_info: None,
            });
        }
    }

    match tokio::time::timeout(timeout, reply_rx).await {
        Ok(Ok(crate::state::ForwarderProxyReply::Response(resp))) => {
            WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
                request_id: req.request_id,
                ok: resp.success,
                error: resp.error,
                reader_info: resp.reader_info,
            })
        }
        Ok(Ok(crate::state::ForwarderProxyReply::Timeout)) => {
            WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
                request_id: req.request_id,
                ok: false,
                error: Some("forwarder timeout".to_owned()),
                reader_info: None,
            })
        }
        Ok(Ok(crate::state::ForwarderProxyReply::InternalError(msg))) => {
            WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
                request_id: req.request_id,
                ok: false,
                error: Some(msg),
                reader_info: None,
            })
        }
        Ok(Err(_)) => {
            WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
                request_id: req.request_id,
                ok: false,
                error: Some("forwarder disconnected".to_owned()),
                reader_info: None,
            })
        }
        Err(_) => {
            WsMessage::ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse {
                request_id: req.request_id,
                ok: false,
                error: Some("timeout waiting for forwarder response".to_owned()),
                reader_info: None,
            })
        }
    }
}
```

- [ ] **Step 2: Add the match arm in the session loop**

In the incoming message match (after the last proxy arm around line 1313), add:

```rust
Ok(WsMessage::ReceiverProxyReaderControlRequest(req)) => {
    pending_proxy_replies
        .spawn(proxy_reader_control_reply(state.clone(), req));
}
```

- [ ] **Step 3: Add `ReceiverProxyReaderControlResponse` to the JoinSet result serialization**

In the pending_proxy_replies result handling section (before the `_ => { continue; }` fallthrough around line 1470), add a match arm:

```rust
WsMessage::ReceiverProxyReaderControlResponse(_) => {
    // handled by generic serialization below
}
```

The primary serialization path at line ~1478 is generic (`serde_json::to_string(&msg)`), so no change needed there. However, there is a per-variant fallback error-handling block at lines 1450-1470 that builds error responses when serialization fails. Add a fallback arm for `ReceiverProxyReaderControlResponse` before the `_ => { continue; }` fallthrough at line 1470, following the same pattern as the other response variants.

- [ ] **Step 4: Run server tests**

Run: `cargo test -p server`
Expected: All existing tests pass. No new tests needed for this task (the proxy handler tests would require a full WS integration setup; manual testing is acceptable).

- [ ] **Step 5: Commit**

```bash
git add services/server/src/ws_receiver.rs
git commit -m "feat(server): add WS proxy handler for receiver reader control"
```

---

## Task 3: Server — Sentinel Tunneling for ReaderInfoUpdate and ReaderDownloadProgress

**Files:**
- Modify: `services/server/src/ws_forwarder.rs` (add sentinel broadcasts in existing handlers)
- Modify: `services/server/src/ws_receiver.rs` (add sentinel intercepts)

**Depends on:** Task 1

### Context

The existing pattern in `ws_forwarder.rs` for `ReaderStatusChanged` (lines 543-574):
1. Build the `WsMessage` variant and serialize to JSON string
2. Get the broadcast tx for the stream via `state.get_or_create_broadcast(stream_id).await`
3. Send a `ReadEvent` with `read_type` = sentinel constant, `raw_frame` = JSON bytes

In `ws_receiver.rs` (lines 1113-1143):
1. Check if `event.read_type.starts_with(SENTINEL_READ_TYPE_PREFIX)`
2. Match on the specific sentinel type
3. Decode `raw_frame` as UTF-8 and send verbatim over the receiver WS socket

### Steps

- [ ] **Step 1: Restructure ReaderInfoUpdate handler to support sentinel broadcast**

In `services/server/src/ws_forwarder.rs`, the existing `ReaderInfoUpdate` handler (lines 608-622) moves `update.reader_ip` and `update.reader_info` into the `DashboardEvent`. We need to restructure to clone before the move, and add the sentinel broadcast. Replace the entire handler block:

```rust
Ok(WsMessage::ReaderInfoUpdate(update)) => {
    let key = crate::state::reader_cache_key(&device_id, &update.reader_ip);
    let cached = CachedReaderState {
        forwarder_id: device_id.clone(),
        reader_ip: update.reader_ip.clone(),
        state: update.state,
        reader_info: update.reader_info.clone(),
    };
    state.reader_states.write().await.insert(key, cached);

    // Tunnel to receivers via sentinel broadcast (before dashboard send moves fields)
    if let Some(stream_id) = stream_map.get(&update.reader_ip) {
        let receiver_update = rt_protocol::ReceiverReaderInfoUpdate {
            stream_id: *stream_id,
            reader_ip: update.reader_ip.clone(),
            state: update.state,
            reader_info: update.reader_info.clone(),
        };
        match serde_json::to_string(
            &rt_protocol::WsMessage::ReceiverReaderInfoUpdate(receiver_update),
        ) {
            Ok(json) => {
                let tx = state.get_or_create_broadcast(*stream_id).await;
                let _ = tx.send(rt_protocol::ReadEvent {
                    forwarder_id: device_id.clone(),
                    reader_ip: update.reader_ip.clone(),
                    stream_epoch: 0,
                    seq: 0,
                    reader_timestamp: String::new(),
                    raw_frame: json.into_bytes(),
                    read_type: rt_protocol::READER_INFO_UPDATED_READ_TYPE.to_owned(),
                });
            }
            Err(e) => {
                error!(
                    device_id = %device_id,
                    reader_ip = %update.reader_ip,
                    error = %e,
                    "failed to serialize ReceiverReaderInfoUpdate for broadcast"
                );
            }
        }
    }

    let _ = state.dashboard_tx.send(DashboardEvent::ReaderInfoUpdated {
        forwarder_id: device_id.clone(),
        reader_ip: update.reader_ip,
        state: update.state,
        reader_info: update.reader_info,
    });
}
```

- [ ] **Step 2: Restructure ReaderDownloadProgress handler to support sentinel broadcast**

Similarly, replace the existing `ReaderDownloadProgress` handler (lines 624-628). The existing handler moves the entire `progress` struct into the dashboard event, so we must build the sentinel before:

```rust
Ok(WsMessage::ReaderDownloadProgress(progress)) => {
    // Tunnel to receivers via sentinel broadcast (before dashboard send moves progress)
    if let Some(stream_id) = stream_map.get(&progress.reader_ip) {
        let receiver_progress = rt_protocol::ReceiverReaderDownloadProgress {
            stream_id: *stream_id,
            reader_ip: progress.reader_ip.clone(),
            state: progress.state,
            reads_received: progress.reads_received,
            progress: progress.progress,
            total: progress.total,
            error: progress.error.clone(),
        };
        match serde_json::to_string(
            &rt_protocol::WsMessage::ReceiverReaderDownloadProgress(receiver_progress),
        ) {
            Ok(json) => {
                let tx = state.get_or_create_broadcast(*stream_id).await;
                let _ = tx.send(rt_protocol::ReadEvent {
                    forwarder_id: device_id.clone(),
                    reader_ip: progress.reader_ip.clone(),
                    stream_epoch: 0,
                    seq: 0,
                    reader_timestamp: String::new(),
                    raw_frame: json.into_bytes(),
                    read_type: rt_protocol::READER_DOWNLOAD_PROGRESS_READ_TYPE.to_owned(),
                });
            }
            Err(e) => {
                error!(
                    device_id = %device_id,
                    reader_ip = %progress.reader_ip,
                    error = %e,
                    "failed to serialize ReceiverReaderDownloadProgress for broadcast"
                );
            }
        }
    }

    let _ = state.dashboard_tx.send(DashboardEvent::ReaderDownloadProgress {
        forwarder_id: device_id.clone(),
        progress,
    });
}
```

- [ ] **Step 3: Add sentinel intercepts in ws_receiver.rs**

In `services/server/src/ws_receiver.rs`, in the sentinel interception block (around line 1116), insert these `else if` branches **between** the `READER_STATUS_CHANGED_READ_TYPE` branch and the `else { warn!("unrecognized sentinel...") }` fallthrough:

```rust
} else if event.read_type == rt_protocol::READER_INFO_UPDATED_READ_TYPE {
    match String::from_utf8(event.raw_frame) {
        Ok(json) => {
            if socket.send(Message::Text(json.into())).await.is_err() {
                warn!(
                    stream_id = %sub.stream_id,
                    "WS send failed for reader_info_updated; closing session"
                );
                return;
            }
        }
        Err(e) => {
            error!(
                stream_id = %sub.stream_id,
                error = %e,
                "invalid UTF-8 in reader_info_updated payload"
            );
        }
    }
} else if event.read_type == rt_protocol::READER_DOWNLOAD_PROGRESS_READ_TYPE {
    match String::from_utf8(event.raw_frame) {
        Ok(json) => {
            if socket.send(Message::Text(json.into())).await.is_err() {
                warn!(
                    stream_id = %sub.stream_id,
                    "WS send failed for reader_download_progress; closing session"
                );
                return;
            }
        }
        Err(e) => {
            error!(
                stream_id = %sub.stream_id,
                error = %e,
                "invalid UTF-8 in reader_download_progress payload"
            );
        }
    }
```

- [ ] **Step 4: Run server tests**

Run: `cargo test -p server`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add services/server/src/ws_forwarder.rs services/server/src/ws_receiver.rs
git commit -m "feat(server): tunnel ReaderInfoUpdate and ReaderDownloadProgress to receivers"
```

---

## Task 4: Receiver Backend — Session, UI Events, Control API, Tauri Commands

**Files:**
- Modify: `services/receiver/src/session.rs` (WsCommand::new, proxy_response_request_id, incoming handling)
- Modify: `services/receiver/src/ui_events.rs` (new ReceiverUiEvent variants)
- Modify: `services/receiver/src/control_api.rs` (new reader control proxy functions)
- Modify: `apps/receiver-ui/src-tauri/src/main.rs` (new Tauri commands + event bridge)

**Depends on:** Tasks 2 and 3

### Steps

- [ ] **Step 1: Add new ReceiverUiEvent variants**

In `services/receiver/src/ui_events.rs`, add two new variants to the `ReceiverUiEvent` enum (after the last variant, before the closing `}`):

```rust
ReaderInfoUpdated {
    stream_id: uuid::Uuid,
    reader_ip: String,
    state: rt_protocol::ReaderConnectionState,
    reader_info: Option<rt_protocol::ReaderInfo>,
},
ReaderDownloadProgress {
    stream_id: uuid::Uuid,
    reader_ip: String,
    state: rt_protocol::DownloadState,
    reads_received: u32,
    progress: u64,
    total: u64,
    error: Option<String>,
},
```

- [ ] **Step 2: Update WsCommand::new() in session.rs**

In `services/receiver/src/session.rs`, in the `WsCommand::new()` match (around line 74), add before the `_ =>` arm:

```rust
WsMessage::ReceiverProxyReaderControlRequest(r) => r.request_id.clone(),
```

- [ ] **Step 3: Update proxy_response_request_id() in session.rs**

In `proxy_response_request_id()` (around line 163), add before the `_ => None` arm:

```rust
WsMessage::ReceiverProxyReaderControlResponse(r) => Some(&r.request_id),
```

- [ ] **Step 4: Add incoming message handlers in session.rs**

In the session loop's incoming message match (before the `Ok(o) => debug!` catch-all around line 340), add:

```rust
Ok(WsMessage::ReceiverReaderInfoUpdate(update)) => {
    let _ = deps.ui_tx.send(ReceiverUiEvent::ReaderInfoUpdated {
        stream_id: update.stream_id,
        reader_ip: update.reader_ip,
        state: update.state,
        reader_info: update.reader_info,
    });
}
Ok(WsMessage::ReceiverReaderDownloadProgress(progress)) => {
    let _ = deps.ui_tx.send(ReceiverUiEvent::ReaderDownloadProgress {
        stream_id: progress.stream_id,
        reader_ip: progress.reader_ip,
        state: progress.state,
        reads_received: progress.reads_received,
        progress: progress.progress,
        total: progress.total,
        error: progress.error,
    });
}
```

- [ ] **Step 5: Add reader control proxy functions in control_api.rs**

In `services/receiver/src/control_api.rs`, add a helper function and 10 public proxy functions. Follow the pattern of `get_forwarder_config` (line 1339).

First, add a generic helper:

```rust
/// Send a reader control action and return the response.
async fn send_reader_control(
    state: &AppState,
    forwarder_id: String,
    reader_ip: String,
    action: rt_protocol::ReaderControlAction,
) -> Result<serde_json::Value, ReceiverError> {
    let msg = rt_protocol::WsMessage::ReceiverProxyReaderControlRequest(
        rt_protocol::ReceiverProxyReaderControlRequest {
            request_id: generate_request_id(),
            forwarder_id,
            reader_ip,
            action,
        },
    );
    let response = send_ws_command(state, msg).await?;
    match response {
        rt_protocol::WsMessage::ReceiverProxyReaderControlResponse(r) => Ok(serde_json::json!({
            "ok": r.ok,
            "error": r.error,
            "reader_info": r.reader_info,
        })),
        _ => Err(ReceiverError::UpstreamError(
            "unexpected response type".to_owned(),
        )),
    }
}
```

Then add 10 public functions:

```rust
pub async fn reader_get_info(
    state: &AppState, forwarder_id: String, reader_ip: String,
) -> Result<serde_json::Value, ReceiverError> {
    send_reader_control(state, forwarder_id, reader_ip, rt_protocol::ReaderControlAction::GetInfo).await
}

pub async fn reader_sync_clock(
    state: &AppState, forwarder_id: String, reader_ip: String,
) -> Result<serde_json::Value, ReceiverError> {
    send_reader_control(state, forwarder_id, reader_ip, rt_protocol::ReaderControlAction::SyncClock).await
}

pub async fn reader_set_read_mode(
    state: &AppState, forwarder_id: String, reader_ip: String, mode: rt_protocol::ReadMode, timeout: u8,
) -> Result<serde_json::Value, ReceiverError> {
    send_reader_control(state, forwarder_id, reader_ip, rt_protocol::ReaderControlAction::SetReadMode { mode, timeout }).await
}

pub async fn reader_set_tto(
    state: &AppState, forwarder_id: String, reader_ip: String, enabled: bool,
) -> Result<serde_json::Value, ReceiverError> {
    send_reader_control(state, forwarder_id, reader_ip, rt_protocol::ReaderControlAction::SetTto { enabled }).await
}

pub async fn reader_set_recording(
    state: &AppState, forwarder_id: String, reader_ip: String, enabled: bool,
) -> Result<serde_json::Value, ReceiverError> {
    send_reader_control(state, forwarder_id, reader_ip, rt_protocol::ReaderControlAction::SetRecording { enabled }).await
}

pub async fn reader_clear_records(
    state: &AppState, forwarder_id: String, reader_ip: String,
) -> Result<serde_json::Value, ReceiverError> {
    send_reader_control(state, forwarder_id, reader_ip, rt_protocol::ReaderControlAction::ClearRecords).await
}

pub async fn reader_start_download(
    state: &AppState, forwarder_id: String, reader_ip: String,
) -> Result<serde_json::Value, ReceiverError> {
    send_reader_control(state, forwarder_id, reader_ip, rt_protocol::ReaderControlAction::StartDownload).await
}

pub async fn reader_stop_download(
    state: &AppState, forwarder_id: String, reader_ip: String,
) -> Result<serde_json::Value, ReceiverError> {
    send_reader_control(state, forwarder_id, reader_ip, rt_protocol::ReaderControlAction::StopDownload).await
}

pub async fn reader_refresh(
    state: &AppState, forwarder_id: String, reader_ip: String,
) -> Result<serde_json::Value, ReceiverError> {
    send_reader_control(state, forwarder_id, reader_ip, rt_protocol::ReaderControlAction::Refresh).await
}

pub async fn reader_reconnect(
    state: &AppState, forwarder_id: String, reader_ip: String,
) -> Result<serde_json::Value, ReceiverError> {
    send_reader_control(state, forwarder_id, reader_ip, rt_protocol::ReaderControlAction::Reconnect).await
}
```

- [ ] **Step 6: Add Tauri command handlers in main.rs**

In `apps/receiver-ui/src-tauri/src/main.rs`, add 10 new `#[tauri::command]` functions following the pattern of `get_forwarder_config` (line 307-314):

```rust
#[tauri::command]
async fn reader_get_info(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
) -> CmdResult<serde_json::Value> {
    control_api::reader_get_info(&state, forwarder_id, reader_ip)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_sync_clock(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
) -> CmdResult<serde_json::Value> {
    control_api::reader_sync_clock(&state, forwarder_id, reader_ip)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_set_read_mode(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
    mode: rt_protocol::ReadMode,
    timeout: u8,
) -> CmdResult<serde_json::Value> {
    control_api::reader_set_read_mode(&state, forwarder_id, reader_ip, mode, timeout)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_set_tto(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
    enabled: bool,
) -> CmdResult<serde_json::Value> {
    control_api::reader_set_tto(&state, forwarder_id, reader_ip, enabled)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_set_recording(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
    enabled: bool,
) -> CmdResult<serde_json::Value> {
    control_api::reader_set_recording(&state, forwarder_id, reader_ip, enabled)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_clear_records(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
) -> CmdResult<serde_json::Value> {
    control_api::reader_clear_records(&state, forwarder_id, reader_ip)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_start_download(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
) -> CmdResult<serde_json::Value> {
    control_api::reader_start_download(&state, forwarder_id, reader_ip)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_stop_download(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
) -> CmdResult<serde_json::Value> {
    control_api::reader_stop_download(&state, forwarder_id, reader_ip)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_refresh(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
) -> CmdResult<serde_json::Value> {
    control_api::reader_refresh(&state, forwarder_id, reader_ip)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn reader_reconnect(
    state: State<'_, Arc<AppState>>,
    forwarder_id: String,
    reader_ip: String,
) -> CmdResult<serde_json::Value> {
    control_api::reader_reconnect(&state, forwarder_id, reader_ip)
        .await
        .map_err(|e| e.to_string())
}
```

- [ ] **Step 7: Register Tauri commands in generate_handler!**

In the `tauri::generate_handler![...]` macro (around line 770-814), add all 10 new commands:

```rust
reader_get_info,
reader_sync_clock,
reader_set_read_mode,
reader_set_tto,
reader_set_recording,
reader_clear_records,
reader_start_download,
reader_stop_download,
reader_refresh,
reader_reconnect,
```

- [ ] **Step 8: Add event bridge mapping for new ReceiverUiEvent variants**

In `main.rs`, in the `ui_event_name()` function (around line 82-94), add:

```rust
ReceiverUiEvent::ReaderInfoUpdated { .. } => "reader_info_updated",
ReceiverUiEvent::ReaderDownloadProgress { .. } => "reader_download_progress",
```

- [ ] **Step 9: Run receiver tests**

Run: `cargo test -p receiver`
Expected: All tests pass.

- [ ] **Step 10: Run clippy on both packages**

Run: `cargo clippy -p receiver -p server -- -D warnings`
Expected: No warnings.

- [ ] **Step 11: Commit**

```bash
git add services/receiver/src/session.rs services/receiver/src/ui_events.rs services/receiver/src/control_api.rs apps/receiver-ui/src-tauri/src/main.rs
git commit -m "feat(receiver): add reader control proxy commands and UI event tunneling"
```

---

## Task 5: Shared UI Component — ReaderControlPanel

**Files:**
- Create: `apps/shared-ui/src/components/ReaderControlPanel.svelte`
- Modify: `apps/shared-ui/src/lib/index.ts` (add export)

**Depends on:** Task 1 (for type awareness only; no Rust dependency)

### Context

The existing reader control UI lives inline in `apps/server-ui/src/routes/+page.svelte` starting at line 813. It uses helpers from `@rusty-timer/shared-ui/lib/reader-view-model` and `@rusty-timer/shared-ui/lib/read-mode-form`. We need to extract this into a reusable component.

The component should accept props for data and callbacks, so each app can wire it to its own backend (HTTP for server-ui, Tauri invoke for receiver-ui).

### Steps

- [ ] **Step 1: Create ReaderControlPanel.svelte**

Create `apps/shared-ui/src/components/ReaderControlPanel.svelte`. The component should:

- Accept these props:
  - `readerIp: string`
  - `readerInfo: ReaderInfo | null` (reader firmware, mode, tto, recording, records, clock drift, etc.)
  - `readerState: string` (ReaderConnectionState — "connected" | "disconnected" | etc.)
  - `downloadProgress: { state: string, reads_received: number, progress: number, total: number, error?: string } | null`
  - `disabled: boolean` (when forwarder offline or reader disconnected)
  - `onSyncClock: () => Promise<void>`
  - `onSetReadMode: (mode: string, timeout: number) => Promise<void>`
  - `onSetTto: (enabled: boolean) => Promise<void>`
  - `onSetRecording: (enabled: boolean) => Promise<void>`
  - `onClearRecords: () => Promise<void>`
  - `onStartDownload: () => Promise<void>`
  - `onStopDownload: () => Promise<void>`
  - `onRefresh: () => Promise<void>`
  - `onReconnect: () => Promise<void>`

- Render the same UI structure as the existing reader control panel on `+page.svelte` (lines 813-1030+):
  - Reader info grid: firmware version, read mode, TTO status, recording status, record count, clock drift with color coding
  - Read mode select (using `READ_MODE_OPTIONS` from `read-mode-form.ts`) + optional timeout input + "Set Mode" button
  - Action buttons row: Sync Clock, Refresh, Reconnect
  - Toggle buttons: TTO, Recording
  - Data operation buttons: Clear Records, Start Download, Stop Download
  - Download progress bar when download is active
  - Inline feedback per action (success/error with auto-dismiss after 3s)

- Use `$state` for local UI state: `busy` flags per action, `feedback` messages, read mode draft, timeout draft
- Import helpers from `../lib/reader-view-model` and `../lib/read-mode-form`
- Use Tailwind classes consistent with existing shared-ui components
- All buttons should be disabled when `disabled` prop is true or when `busy` for that action

Reference the existing implementation at `apps/server-ui/src/routes/+page.svelte` lines 813-1030+ for the exact layout, class names, and interaction patterns. Extract, don't reinvent.

- [ ] **Step 2: Export from shared-ui index**

In `apps/shared-ui/src/lib/index.ts`, add:

```typescript
export { default as ReaderControlPanel } from "../components/ReaderControlPanel.svelte";
```

- [ ] **Step 3: Commit**

```bash
git add apps/shared-ui/src/components/ReaderControlPanel.svelte apps/shared-ui/src/lib/index.ts
git commit -m "feat(shared-ui): add ReaderControlPanel component"
```

---

## Task 6: Server UI — Reader Control on Stream Detail Page

**Files:**
- Modify: `apps/server-ui/src/routes/streams/[streamId]/+page.svelte`

**Depends on:** Task 5

### Context

The stream detail page currently has (top to bottom): Header, Info + Metrics cards (2-col), Epoch Race Mapping card, Reads card, Export card. We add a Reader Control card above Epoch Race Mapping.

The page already has `forwarder_id` and `reader_ip` from the stream data. Reader state comes from `$readerStatesStore` and download progress from `$downloadProgressStore` — both are already imported in `+page.svelte` or available from `$lib/stores`.

### Steps

- [ ] **Step 1: Import the ReaderControlPanel and reader control API functions**

At the top of `apps/server-ui/src/routes/streams/[streamId]/+page.svelte`, add imports. These are NOT currently imported on this page — add them explicitly:

```svelte
<script>
  import { ReaderControlPanel } from "@rusty-timer/shared-ui";
  import {
    syncReaderClock, setReaderReadMode, setReaderTto, setReaderRecording,
    clearReaderRecords, startReaderDownload, stopReaderDownload,
    refreshReader, reconnectReader,
  } from "$lib/api";
  import { readerStatesStore, downloadProgressStore } from "$lib/stores";
</script>
```

Note: The `readerKey` helper is defined locally in `+page.svelte` (main page) — define it locally here too as `const rKey = \`${forwarderId}:${readerIp}\``.

- [ ] **Step 2: Add the Reader Control card**

Before the Epoch Race Mapping card (line 660), add:

```svelte
{#if stream}
  {@const rKey = `${stream.forwarder_id}:${stream.reader_ip}`}
  {@const rs = $readerStatesStore[rKey]}
  {@const dp = $downloadProgressStore[rKey]}
  <Card title="Reader Control">
    <ReaderControlPanel
      readerIp={stream.reader_ip}
      readerInfo={rs?.reader_info ?? null}
      readerState={rs?.state ?? "disconnected"}
      downloadProgress={dp ?? null}
      disabled={!stream.online || rs?.state !== "connected"}
      onSyncClock={() => syncReaderClock(stream.forwarder_id, stream.reader_ip)}
      onSetReadMode={(mode, timeout) => setReaderReadMode(stream.forwarder_id, stream.reader_ip, mode, timeout)}
      onSetTto={(enabled) => setReaderTto(stream.forwarder_id, stream.reader_ip, enabled)}
      onSetRecording={(enabled) => setReaderRecording(stream.forwarder_id, stream.reader_ip, enabled)}
      onClearRecords={() => clearReaderRecords(stream.forwarder_id, stream.reader_ip)}
      onStartDownload={() => startReaderDownload(stream.forwarder_id, stream.reader_ip)}
      onStopDownload={() => stopReaderDownload(stream.forwarder_id, stream.reader_ip)}
      onRefresh={() => refreshReader(stream.forwarder_id, stream.reader_ip)}
      onReconnect={() => reconnectReader(stream.forwarder_id, stream.reader_ip)}
    />
  </Card>
{/if}
```

Note: Verify the exact variable names used on this page for the stream data (`stream` vs `streamData` etc.) and stores. Adjust accordingly.

- [ ] **Step 3: Test manually**

1. Run the server-ui dev server: `cd apps/server-ui && npm run dev`
2. Navigate to a stream detail page
3. Verify the Reader Control card appears above Epoch Race Mapping
4. Verify all buttons work when a forwarder is connected

- [ ] **Step 4: Commit**

```bash
git add apps/server-ui/src/routes/streams/\[streamId\]/+page.svelte
git commit -m "feat(server-ui): add reader control card to stream detail page"
```

---

## Task 7: Receiver UI — Reader Control in Streams Tab + Event Wiring

**Files:**
- Modify: `apps/receiver-ui/src/lib/api.ts` (new invoke functions)
- Modify: `apps/receiver-ui/src/lib/sse.ts` (new event listeners)
- Modify: `apps/receiver-ui/src/lib/store.svelte.ts` (store handlers for new events)
- Modify: `apps/receiver-ui/src/lib/components/StreamsTab.svelte` (add ReaderControlPanel in expanded row)

**Depends on:** Tasks 4 and 5

### Steps

- [ ] **Step 1: Add API invoke functions**

In `apps/receiver-ui/src/lib/api.ts`, add:

```typescript
export async function readerGetInfo(forwarderId: string, readerIp: string) {
  return invoke<{ ok: boolean; error?: string; reader_info?: unknown }>("reader_get_info", { forwarderId, readerIp });
}

export async function readerSyncClock(forwarderId: string, readerIp: string) {
  return invoke<{ ok: boolean; error?: string; reader_info?: unknown }>("reader_sync_clock", { forwarderId, readerIp });
}

export async function readerSetReadMode(forwarderId: string, readerIp: string, mode: string, timeout: number) {
  return invoke<{ ok: boolean; error?: string; reader_info?: unknown }>("reader_set_read_mode", { forwarderId, readerIp, mode, timeout });
}

export async function readerSetTto(forwarderId: string, readerIp: string, enabled: boolean) {
  return invoke<{ ok: boolean; error?: string; reader_info?: unknown }>("reader_set_tto", { forwarderId, readerIp, enabled });
}

export async function readerSetRecording(forwarderId: string, readerIp: string, enabled: boolean) {
  return invoke<{ ok: boolean; error?: string; reader_info?: unknown }>("reader_set_recording", { forwarderId, readerIp, enabled });
}

export async function readerClearRecords(forwarderId: string, readerIp: string) {
  return invoke<{ ok: boolean; error?: string }>("reader_clear_records", { forwarderId, readerIp });
}

export async function readerStartDownload(forwarderId: string, readerIp: string) {
  return invoke<{ ok: boolean; error?: string }>("reader_start_download", { forwarderId, readerIp });
}

export async function readerStopDownload(forwarderId: string, readerIp: string) {
  return invoke<{ ok: boolean; error?: string }>("reader_stop_download", { forwarderId, readerIp });
}

export async function readerRefresh(forwarderId: string, readerIp: string) {
  return invoke<{ ok: boolean; error?: string; reader_info?: unknown }>("reader_refresh", { forwarderId, readerIp });
}

export async function readerReconnect(forwarderId: string, readerIp: string) {
  return invoke<{ ok: boolean; error?: string }>("reader_reconnect", { forwarderId, readerIp });
}
```

- [ ] **Step 2: Add event listeners in sse.ts**

In `apps/receiver-ui/src/lib/sse.ts`, in the `initSSE(callbacks)` function, add two new `listen()` calls inside the `Promise.all([...])` array:

```typescript
listen("reader_info_updated", (event) => {
  callbacks.onReaderInfoUpdated?.(event.payload as {
    stream_id: string;
    reader_ip: string;
    state: string;
    reader_info: unknown | null;
  });
}),
listen("reader_download_progress", (event) => {
  callbacks.onReaderDownloadProgress?.(event.payload as {
    stream_id: string;
    reader_ip: string;
    state: string;
    reads_received: number;
    progress: number;
    total: number;
    error?: string;
  });
}),
```

Update the callbacks type to include:
```typescript
onReaderInfoUpdated?: (data: { stream_id: string; reader_ip: string; state: string; reader_info: unknown | null }) => void;
onReaderDownloadProgress?: (data: { stream_id: string; reader_ip: string; state: string; reads_received: number; progress: number; total: number; error?: string }) => void;
```

- [ ] **Step 3: Add store handlers**

In `apps/receiver-ui/src/lib/store.svelte.ts`:

1. Add new `$state` fields to the store for reader state and download progress, keyed by a composite key (e.g., `forwarderId:readerIp`). Check if the store already has a `readerStates` or similar field — if so, update that. Otherwise add:

```typescript
readerStates: {} as Record<string, { state: string; reader_info: unknown | null }>,
downloadProgress: {} as Record<string, { state: string; reads_received: number; progress: number; total: number; error?: string }>,
```

2. In the `initSSE({...})` callbacks object (around line 1122), add handlers:

```typescript
onReaderInfoUpdated: (data) => {
  // Key by forwarder:reader — need to look up which forwarder owns this stream_id
  // Or key by stream_id directly if that's simpler
  store.readerStates[data.reader_ip] = {
    state: data.state,
    reader_info: data.reader_info,
  };
},
onReaderDownloadProgress: (data) => {
  store.downloadProgress[data.reader_ip] = {
    state: data.state,
    reads_received: data.reads_received,
    progress: data.progress,
    total: data.total,
    error: data.error,
  };
},
```

Note: The exact keying strategy should match what `StreamsTab.svelte` will use to look up the state. Check how the existing streams data is keyed in the store and use the same pattern.

- [ ] **Step 4: Add ReaderControlPanel to StreamsTab expanded row**

In `apps/receiver-ui/src/lib/components/StreamsTab.svelte`, in the expanded row section (after the metrics grid around line 387, before the action row around line 392):

```svelte
<script>
  import { ReaderControlPanel } from "@rusty-timer/shared-ui";
  import {
    readerSyncClock, readerSetReadMode, readerSetTto, readerSetRecording,
    readerClearRecords, readerStartDownload, readerStopDownload,
    readerRefresh, readerReconnect,
  } from "$lib/api";
</script>

<!-- Inside the expanded row, after metrics grid -->
<div class="mt-3 border-t border-border pt-3">
  <ReaderControlPanel
    readerIp={stream.reader_ip}
    readerInfo={store.readerStates[stream.reader_ip]?.reader_info ?? null}
    readerState={stream.reader_connected ? "connected" : "disconnected"}
    downloadProgress={store.downloadProgress[stream.reader_ip] ?? null}
    disabled={!stream.online || !stream.reader_connected}
    onSyncClock={() => readerSyncClock(stream.forwarder_id, stream.reader_ip)}
    onSetReadMode={(mode, timeout) => readerSetReadMode(stream.forwarder_id, stream.reader_ip, mode, timeout)}
    onSetTto={(enabled) => readerSetTto(stream.forwarder_id, stream.reader_ip, enabled)}
    onSetRecording={(enabled) => readerSetRecording(stream.forwarder_id, stream.reader_ip, enabled)}
    onClearRecords={() => readerClearRecords(stream.forwarder_id, stream.reader_ip)}
    onStartDownload={() => readerStartDownload(stream.forwarder_id, stream.reader_ip)}
    onStopDownload={() => readerStopDownload(stream.forwarder_id, stream.reader_ip)}
    onRefresh={() => readerRefresh(stream.forwarder_id, stream.reader_ip)}
    onReconnect={() => readerReconnect(stream.forwarder_id, stream.reader_ip)}
  />
</div>
```

Note: Verify the exact variable names available in scope (`stream` fields like `forwarder_id`, `reader_ip`, `online`, `reader_connected`). Adjust the `disabled` logic and store key lookups to match the actual data shape.

- [ ] **Step 5: Fetch reader info on expand**

When a row is expanded, trigger a `readerGetInfo` call to populate the initial reader state. In the `toggleExpand` function (around line 58), add:

```typescript
function toggleExpand(key: string, forwarderId: string, readerIp: string) {
  if (expandedKey === key) {
    expandedKey = null;
  } else {
    expandedKey = key;
    // Fetch initial reader info
    readerGetInfo(forwarderId, readerIp).then((resp) => {
      if (resp.ok && resp.reader_info) {
        store.readerStates[readerIp] = {
          state: "connected",
          reader_info: resp.reader_info,
        };
      }
    }).catch(() => { /* ignore — reader may be offline */ });
  }
}
```

Adjust the `toggleExpand` call sites to pass the additional arguments.

- [ ] **Step 6: Test manually**

1. Run the receiver-ui: `cd apps/receiver-ui && npm run tauri dev`
2. Connect to a server with a forwarder online
3. Expand a stream row — verify reader control panel appears
4. Test each action button
5. Verify real-time updates when reader info changes or download progresses

- [ ] **Step 7: Commit**

```bash
git add apps/receiver-ui/src/lib/api.ts apps/receiver-ui/src/lib/sse.ts apps/receiver-ui/src/lib/store.svelte.ts apps/receiver-ui/src/lib/components/StreamsTab.svelte
git commit -m "feat(receiver-ui): add reader control to streams tab with live updates"
```
