# Server Reader Control — PR Review Fixes Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix all critical, important, test coverage, and suggested issues from the PR #147 review.

**Architecture:** Bottom-up fixes across 6 phases: protocol enums → server → forwarder → frontend → docs → integration tests. Each phase is a separate commit.

**Tech Stack:** Rust (rt-protocol, server, forwarder), TypeScript/Svelte (server-ui, shared-ui), Vitest, testcontainers-rs

---

### Task 1: Add protocol enums (Phase 1)

**Files:**
- Modify: `crates/rt-protocol/src/lib.rs:307-404` (struct/enum definitions)
- Modify: `crates/rt-protocol/src/lib.rs:504-582` (round-trip tests)

**Step 1: Add `ReadMode` enum**

At `crates/rt-protocol/src/lib.rs`, before `HardwareInfo` (line ~307), add:

```rust
/// IPICO reader read mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadMode {
    #[serde(rename = "raw")]
    Raw,
    #[serde(rename = "event")]
    Event,
    #[serde(rename = "fsls")]
    FirstLastSeen,
}
```

Note: This mirrors `ipico_core::control::ReadMode` but lives in the protocol crate for wire-level typing. Use explicit `#[serde(rename)]` per-variant (not `rename_all`) because `FirstLastSeen` → `"fsls"` is a custom mapping.

**Step 2: Add `ReaderConnectionState` enum**

```rust
/// Reader connection state as reported by the forwarder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReaderConnectionState {
    Connected,
    Connecting,
    Disconnected,
}
```

**Step 3: Add `DownloadState` enum**

```rust
/// State of a reader download operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Downloading,
    Complete,
    Error,
    Idle,
}
```

**Step 4: Update structs to use enums**

In `Config3Info` (line ~315-319), change `mode: String` to `mode: ReadMode`:
```rust
pub struct Config3Info {
    pub mode: ReadMode,
    pub timeout: u8,
}
```

In `ReaderControlAction::SetReadMode` (line ~355), change `mode: String` to `mode: ReadMode`:
```rust
SetReadMode { mode: ReadMode, timeout: u8 },
```

In `ReaderInfoUpdate` (line ~386-390), change `state: String` to `state: ReaderConnectionState`:
```rust
pub struct ReaderInfoUpdate {
    pub reader_ip: String,
    pub state: ReaderConnectionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reader_info: Option<ReaderInfo>,
}
```

In `ReaderDownloadProgress` (line ~392-404), change `state: String` to `state: DownloadState`:
```rust
pub struct ReaderDownloadProgress {
    pub reader_ip: String,
    pub state: DownloadState,
    pub reads_received: u32,
    pub progress: u64,
    pub total: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
```

**Step 5: Update serde round-trip tests**

In `reader_control_request_round_trip` (line ~509-521), change:
```rust
action: ReaderControlAction::SetReadMode {
    mode: "event".into(),
    timeout: 5,
},
```
to:
```rust
action: ReaderControlAction::SetReadMode {
    mode: ReadMode::Event,
    timeout: 5,
},
```

In `reader_info_update_round_trip` (line ~556-566), change `state: "connected".into()` to `state: ReaderConnectionState::Connected`.

In `reader_download_progress_round_trip` (line ~568-581), change `state: "downloading".into()` to `state: DownloadState::Downloading`.

**Step 6: Run tests**

Run: `cargo test -p rt-protocol`
Expected: All 4 round-trip tests pass. Serde produces identical JSON since renames match the old string values.

**Step 7: Fix server compilation — `CachedReaderState`**

In `services/server/src/state.rs:43-49`, change `state: String` to `state: rt_protocol::ReaderConnectionState`:
```rust
pub struct CachedReaderState {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub state: rt_protocol::ReaderConnectionState,
    pub reader_info: Option<rt_protocol::ReaderInfo>,
}
```

**Step 8: Fix server compilation — `DashboardEvent`**

In `services/server/src/dashboard_events.rs:66-82`:
- `ReaderInfoUpdated.state`: change `String` to `rt_protocol::ReaderConnectionState`
- `ReaderDownloadProgress.state`: change `String` to `rt_protocol::DownloadState`

**Step 9: Fix server compilation — `ws_forwarder.rs`**

In the `ReaderInfoUpdate` handler (line ~458-473), the `CachedReaderState` construction changes:
```rust
let cached = CachedReaderState {
    forwarder_id: device_id.clone(),
    reader_ip: update.reader_ip.clone(),
    state: update.state,  // now ReaderConnectionState, not String
    reader_info: update.reader_info.clone(),
};
```

In the disconnect cleanup (line ~581-598), change `state: "disconnected".into()` to `state: rt_protocol::ReaderConnectionState::Disconnected`.

**Step 10: Fix forwarder compilation — `main.rs`**

In the `SetReadMode` arm (line ~837-852), replace string matching with direct conversion:
```rust
ReaderControlAction::SetReadMode { mode, timeout } => {
    let read_mode = match mode {
        rt_protocol::ReadMode::Raw => ipico_core::control::ReadMode::Raw,
        rt_protocol::ReadMode::Event => ipico_core::control::ReadMode::Event,
        rt_protocol::ReadMode::FirstLastSeen => ipico_core::control::ReadMode::FirstLastSeen,
    };
    // ... rest of handler using read_mode ...
```
This eliminates the `other => { error response }` fallback arm.

In the `ReaderUpdated` handler (line ~1513-1530), change string construction to use the protocol enum:
```rust
Ok(ForwarderUiEvent::ReaderUpdated { ip, state, .. }) => {
    let proto_state = match state {
        forwarder::ui_events::ReaderConnectionState::Connected => rt_protocol::ReaderConnectionState::Connected,
        forwarder::ui_events::ReaderConnectionState::Connecting => rt_protocol::ReaderConnectionState::Connecting,
        forwarder::ui_events::ReaderConnectionState::Disconnected => rt_protocol::ReaderConnectionState::Disconnected,
    };
    let msg = WsMessage::ReaderInfoUpdate(rt_protocol::ReaderInfoUpdate {
        reader_ip: ip.clone(),
        state: proto_state,
        reader_info: None,
    });
```

In the `ReaderInfoUpdated` handler (line ~1531-1541), change `state: "connected".to_owned()` to `state: rt_protocol::ReaderConnectionState::Connected`.

**Step 11: Build and test all**

Run: `cargo build --workspace && cargo test --workspace`
Expected: All compile, all tests pass.

**Step 12: Commit**

```bash
git add crates/rt-protocol/src/lib.rs services/server/src/state.rs services/server/src/dashboard_events.rs services/server/src/ws_forwarder.rs services/forwarder/src/main.rs
git commit -m "refactor(protocol): replace stringly-typed fields with enums

Add ReadMode, ReaderConnectionState, and DownloadState enums to
rt-protocol. Update all structs and consumers. Wire format unchanged
(serde renames match previous string values)."
```

---

### Task 2: Server Rust fixes (Phase 2)

**Files:**
- Modify: `services/server/src/state.rs:14-41` (ForwarderProxyReply, ForwarderCommand)
- Modify: `services/server/src/ws_forwarder.rs:548-565` (serialize+send block)
- Modify: `services/server/src/http/reader_control.rs` (entire file)
- Modify: `services/server/src/dashboard_events.rs:73-82` (flatten)
- Modify: `services/server/src/http/sse.rs` (download progress match)

**Step 1: Add `ForwarderProxyReply::InternalError` variant (issue 4)**

In `services/server/src/state.rs:14-17`:
```rust
pub enum ForwarderProxyReply<T> {
    Response(T),
    Timeout,
    InternalError(String),
}
```

**Step 2: Add `ForwarderCommand::ReaderControlFireAndForget` variant (issue 5)**

In `services/server/src/state.rs`, after the existing `ReaderControl` variant (line ~35-40), add:
```rust
ReaderControlFireAndForget {
    reader_ip: String,
    action: rt_protocol::ReaderControlAction,
},
```
No `request_id` or `reply` — fire and forget.

**Step 3: Add `reader_cache_key` helper (issue 14)**

In `services/server/src/state.rs`, add:
```rust
/// Composite cache key for reader state entries.
pub fn reader_cache_key(forwarder_id: &str, reader_ip: &str) -> String {
    format!("{}:{}", forwarder_id, reader_ip)
}
```

**Step 4: Add `validate_reader_ip` helper (issue 9)**

In `services/server/src/http/reader_control.rs`, add near the top:
```rust
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
```

**Step 5: Update `send_reader_control` to validate + log (issues 8, 9)**

In `services/server/src/http/reader_control.rs`, at the start of `send_reader_control` (line ~26), add validation and logging:
```rust
validate_reader_ip(&reader_ip)?;
tracing::debug!(forwarder_id = %forwarder_id, reader_ip = %reader_ip, action = ?action, "sending reader control request");
```

At the response handling, add a warning for failures:
- After the `Timeout` arm (line ~61): already returns error, add `tracing::warn!` before returning
- After the `InternalError` arm (new): map to HTTP 500
```rust
ForwarderProxyReply::InternalError(msg) => {
    tracing::error!(forwarder_id = %forwarder_id, error = %msg, "internal error in reader control proxy");
    Err((
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({
            "code": "INTERNAL_ERROR",
            "message": msg,
        })),
    ))
}
```

**Step 6: Update `send_fire_and_forget` to validate + log + use new variant (issues 5, 8, 9)**

Rewrite `send_fire_and_forget` to use `ForwarderCommand::ReaderControlFireAndForget`:
```rust
validate_reader_ip(&reader_ip)?;
tracing::debug!(forwarder_id = %forwarder_id, reader_ip = %reader_ip, action = ?action, "sending fire-and-forget reader control");

let sender = {
    let senders = state.forwarder_command_senders.read().await;
    senders.get(&forwarder_id).cloned()
};
let Some(sender) = sender else {
    return Err(not_found("forwarder not connected"));
};

let cmd = ForwarderCommand::ReaderControlFireAndForget {
    reader_ip,
    action,
};
match sender.try_send(cmd) {
    Ok(()) => Ok((StatusCode::ACCEPTED, Json(serde_json::json!({ "ok": true })))),
    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
        tracing::warn!(forwarder_id = %forwarder_id, "fire-and-forget: command queue saturated");
        Err(gateway_timeout())
    }
    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
        Err(not_found("forwarder not connected"))
    }
}
```

**Step 7: Handle `ForwarderCommand::ReaderControlFireAndForget` in `ws_forwarder.rs`**

In the command receive match (around line ~548), add a new arm after the existing `ReaderControl` arm:
```rust
ForwarderCommand::ReaderControlFireAndForget { reader_ip, action } => {
    let msg = WsMessage::ReaderControlRequest(rt_protocol::ReaderControlRequest {
        request_id: uuid::Uuid::new_v4().to_string(),
        reader_ip,
        action,
    });
    match serde_json::to_string(&msg) {
        Ok(json) => {
            if let Err(e) = socket.send(Message::Text(json.into())).await {
                warn!(error = %e, "failed to send fire-and-forget reader control");
            }
            // No pending entry — response will be silently ignored
        }
        Err(e) => {
            error!(device_id = %device_id, error = %e, "failed to serialize fire-and-forget reader control request");
        }
    }
}
```

**Step 8: Fix `ForwarderProxyReply::InternalError` in serialize failure path**

In `ws_forwarder.rs:548-565`, in the existing `ReaderControl` arm, change the serialization failure from:
```rust
warn!(device_id = %device_id, "failed to serialize reader control request");
let _ = reply.send(ForwarderProxyReply::Timeout);
```
to:
```rust
let err_msg = format!("failed to serialize reader control request: {}", e);
error!(device_id = %device_id, error = %e, "failed to serialize reader control request");
let _ = reply.send(ForwarderProxyReply::InternalError(err_msg));
```
Note: change `if let Ok(json) = serde_json::to_string(&msg)` to a proper `match` to capture the error.

**Step 9: Use `reader_cache_key` in `ws_forwarder.rs`**

Replace the inline `format!("{}:{}", device_id, update.reader_ip)` at line ~466 with:
```rust
use crate::state::reader_cache_key;
let key = reader_cache_key(&device_id, &update.reader_ip);
```

Also in the disconnect cleanup (line ~583-585):
```rust
let prefix = format!("{}:", device_id);
```
stays as-is since it's a prefix match, not a full key.

**Step 10: Use `#[serde(flatten)]` in `DashboardEvent::ReaderDownloadProgress` (issue 15)**

In `services/server/src/dashboard_events.rs:73-82`, replace:
```rust
ReaderDownloadProgress {
    forwarder_id: String,
    reader_ip: String,
    state: rt_protocol::DownloadState,
    reads_received: u32,
    progress: u64,
    total: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
},
```
with:
```rust
ReaderDownloadProgress {
    forwarder_id: String,
    #[serde(flatten)]
    progress: rt_protocol::ReaderDownloadProgress,
},
```

Then update the construction site in `ws_forwarder.rs` (line ~474-484):
```rust
Ok(WsMessage::ReaderDownloadProgress(progress)) => {
    let _ = state.dashboard_tx.send(DashboardEvent::ReaderDownloadProgress {
        forwarder_id: device_id.clone(),
        progress,
    });
}
```

And update the SSE match arm in `services/server/src/http/sse.rs` that maps this variant — the event type string `"reader_download_progress"` stays the same but the match destructuring changes to `DashboardEvent::ReaderDownloadProgress { forwarder_id, progress }`.

**Step 11: Build and test**

Run: `cargo build --workspace && cargo test --workspace`
Expected: All compile, all tests pass.

**Step 12: Commit**

```bash
git add services/server/src/state.rs services/server/src/ws_forwarder.rs services/server/src/http/reader_control.rs services/server/src/dashboard_events.rs services/server/src/http/sse.rs
git commit -m "fix(server): improve reader control error handling, logging, and validation

- Add ForwarderProxyReply::InternalError for serialization failures
- Add ForwarderCommand::ReaderControlFireAndForget (no pending entry leak)
- Add reader_ip validation (400 on invalid format)
- Add tracing::debug/warn for reader control requests
- Centralize reader_cache_key helper
- Use serde(flatten) in DashboardEvent::ReaderDownloadProgress"
```

---

### Task 3: Forwarder Rust fixes (Phase 3)

**Files:**
- Modify: `services/forwarder/src/main.rs:1150-1262` (SyncClock arm)
- Modify: `services/forwarder/src/main.rs:1531-1541` (ReaderInfoUpdated handler)

**Step 1: Spawn SyncClock as background task (issue 6)**

In `services/forwarder/src/main.rs`, replace the `SyncClock` arm (line ~1150-1262) with the background-spawn pattern matching `ClearRecords`. The key change: move all the RTT probes, `set_date_time`, and verification into a `tokio::spawn` block, and return success immediately.

```rust
ReaderControlAction::SyncClock => {
    let bg_client = client.clone();
    let bg_status = status.clone();
    let bg_reader_ip = reader_ip.clone();
    let bg_logger = logger.clone();
    tokio::spawn(async move {
        const SYNC_DELAY_MS: u64 = 500;

        // RTT probes
        let mut rtts = Vec::new();
        for _ in 0..3 {
            let start = Instant::now();
            if bg_client.get_date_time().await.is_ok() {
                rtts.push(start.elapsed());
            }
        }
        if rtts.is_empty() {
            warn!(reader_ip = %bg_reader_ip, "sync_clock: all RTT probes failed");
            return;
        }
        rtts.sort();
        let one_way = rtts[rtts.len() / 2] / 2;

        // Compute timing and set clock
        let wall_now = chrono::Utc::now();
        let (target_boundary, pre_set_wait) =
            forwarder::reader_control::compute_sync_timing(wall_now, one_way, SYNC_DELAY_MS);
        tokio::time::sleep(pre_set_wait).await;

        let (year, month, day, hour, minute, second) = (
            target_boundary.year() as u16,
            target_boundary.month() as u8,
            target_boundary.day() as u8,
            target_boundary.hour() as u8,
            target_boundary.minute() as u8,
            target_boundary.second() as u8,
        );
        let dow = target_boundary.weekday().num_days_from_sunday() as u8;

        if let Err(e) = bg_client
            .set_date_time(year, month, day, dow, hour, minute, second)
            .await
        {
            warn!(reader_ip = %bg_reader_ip, error = %e, "sync_clock: set_date_time failed");
            // Clear clock info in cache
            let mut info = bg_status
                .get_reader_info(&bg_reader_ip)
                .await
                .unwrap_or_default();
            info.clock = None;
            bg_status
                .update_reader_info_unless_disconnected(&bg_reader_ip, info)
                .await;
            return;
        }

        // Verify
        let verify_wait = Duration::from_millis(SYNC_DELAY_MS) + one_way;
        tokio::time::sleep(verify_wait).await;

        match bg_client.get_date_time().await {
            Ok(dt_resp) => {
                let reader_clock_str = format!(
                    "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
                    dt_resp.year, dt_resp.month, dt_resp.day,
                    dt_resp.hour, dt_resp.minute, dt_resp.second
                );
                let reader_dt = chrono::NaiveDateTime::parse_from_str(
                    &reader_clock_str,
                    "%Y-%m-%dT%H:%M:%S",
                );
                let drift_ms = reader_dt
                    .map(|rdt| {
                        let now = chrono::Utc::now().naive_utc();
                        (rdt - now).num_milliseconds()
                    })
                    .unwrap_or(0);

                let mut info = bg_status
                    .get_reader_info(&bg_reader_ip)
                    .await
                    .unwrap_or_default();
                info.clock = Some(forwarder::reader_control::ClockInfo {
                    reader_clock: reader_clock_str,
                    drift_ms,
                });
                bg_status
                    .update_reader_info_unless_disconnected(&bg_reader_ip, info)
                    .await;
                bg_logger.log(format!("sync_clock complete for {}, drift={}ms", bg_reader_ip, drift_ms));
            }
            Err(e) => {
                warn!(reader_ip = %bg_reader_ip, error = %e, "sync_clock: verification get_date_time failed");
            }
        }
    });

    // Return success immediately
    let response = WsMessage::ReaderControlResponse(rt_protocol::ReaderControlResponse {
        request_id,
        reader_ip,
        success: true,
        error: None,
        reader_info: None,
    });
    session.send_message(&response).await
}
```

Note: The background task pushes updated info via `update_reader_info_unless_disconnected`, which already emits a `ForwarderUiEvent::ReaderInfoUpdated` that the uplink loop forwards to the server as `ReaderInfoUpdate`. So the dashboard will get the clock info via SSE automatically.

**Step 2: Look up actual reader state in `ReaderInfoUpdated` handler (issue 7)**

In `services/forwarder/src/main.rs:1531-1541`, change:
```rust
Ok(ForwarderUiEvent::ReaderInfoUpdated { ip, info }) => {
    let msg = WsMessage::ReaderInfoUpdate(rt_protocol::ReaderInfoUpdate {
        reader_ip: ip.clone(),
        state: "connected".to_owned(),
        reader_info: Some(to_protocol_reader_info(&info)),
    });
```
to:
```rust
Ok(ForwarderUiEvent::ReaderInfoUpdated { ip, info }) => {
    let proto_state = {
        let ss = subsystem.lock().await;
        ss.readers.get(&ip).map(|r| match r.state {
            ReaderConnectionState::Connected => rt_protocol::ReaderConnectionState::Connected,
            ReaderConnectionState::Connecting => rt_protocol::ReaderConnectionState::Connecting,
            ReaderConnectionState::Disconnected => rt_protocol::ReaderConnectionState::Disconnected,
        }).unwrap_or(rt_protocol::ReaderConnectionState::Connected)
    };
    let msg = WsMessage::ReaderInfoUpdate(rt_protocol::ReaderInfoUpdate {
        reader_ip: ip.clone(),
        state: proto_state,
        reader_info: Some(to_protocol_reader_info(&info)),
    });
```

Note: `subsystem` is the `Arc<Mutex<SubsystemStatus>>` already available in the `run_uplink` function scope (parameter at line 1279). `ReaderConnectionState` here refers to `forwarder::status_http::ReaderConnectionState` (imported at line 11).

**Step 3: Build and test**

Run: `cargo build --workspace && cargo test --workspace`
Expected: All pass.

**Step 4: Commit**

```bash
git add services/forwarder/src/main.rs
git commit -m "fix(forwarder): spawn SyncClock as background task, look up actual reader state

- SyncClock no longer blocks the uplink loop (3-5s). Follows the
  ClearRecords/StartDownload background-spawn pattern.
- ReaderInfoUpdated events now look up actual reader connection state
  from SubsystemStatus instead of hardcoding 'connected'."
```

---

### Task 4: Frontend fixes (Phase 4)

**Files:**
- Modify: `apps/server-ui/src/lib/api.ts:758-801` (types)
- Modify: `apps/server-ui/src/lib/sse.ts:57-131` (try-catch + types)
- Modify: `apps/server-ui/src/routes/+page.svelte:69-76, 923-932` (cast, button)
- Modify: `apps/server-ui/src/lib/sse.test.ts` (update test expectations if needed)

**Step 1: Fix TypeScript types (issues 1, 17)**

In `apps/server-ui/src/lib/api.ts`, replace:

```typescript
// api.ts:758-761  — use string literal union
export interface Config3Info {
  mode: "raw" | "event" | "fsls";
  timeout: number;
}
```

```typescript
// api.ts:778-783 — use string literal union for state
export interface CachedReaderState {
  forwarder_id: string;
  reader_ip: string;
  state: "connected" | "connecting" | "disconnected";
  reader_info: ReaderInfo | null;
}
```

Add a new type for the actual HTTP response shape and keep the protocol type for reference:

```typescript
// Replace the existing ReaderControlResponse (api.ts:785-791) with:
/** HTTP response shape from reader control endpoints (differs from WS protocol type). */
export interface ReaderControlHttpResponse {
  ok: boolean;
  error: string | null;
  reader_info: ReaderInfo | null;
}
```

```typescript
// api.ts:793-801 — use string literal union for state
export interface ReaderDownloadProgressEvent {
  forwarder_id: string;
  reader_ip: string;
  state: "downloading" | "complete" | "error" | "idle";
  reads_received: number;
  progress: number;
  total: number;
  error?: string | null;
}
```

**Step 2: Update API function return types**

Search `api.ts` for `apiFetch<ReaderControlResponse>` calls and change them to `apiFetch<ReaderControlHttpResponse>`. These are the reader control API functions (e.g., `getReaderInfo`, `syncClock`, `setReadMode`, etc.).

**Step 3: Fix Reconnect button (issue 2)**

In `apps/server-ui/src/routes/+page.svelte:931`, change:
```svelte
disabled={busy}>Reconnect</button
```
to:
```svelte
disabled={busy || !stream.online}>Reconnect</button
```

**Step 4: Remove unnecessary cast in `readModeDraftValue` (issue 17)**

In `apps/server-ui/src/routes/+page.svelte:69-76`, since `Config3Info.mode` is now `"raw" | "event" | "fsls"`, simplify:
```typescript
function readModeDraftValue(
  key: string,
  info: api.ReaderInfo | null | undefined,
): "raw" | "event" | "fsls" {
  return (readModeDrafts[key] as "raw" | "event" | "fsls" | undefined) ??
    info?.config?.mode ??
    "raw";
}
```
The outer `as "raw" | "event" | "fsls"` cast is no longer needed since `info?.config?.mode` is already the correct type. Keep the inner cast on `readModeDrafts[key]` since `readModeDrafts` is a `Record<string, string>`.

**Step 5: Backfill try-catch to older SSE handlers (issue 16)**

In `apps/server-ui/src/lib/sse.ts`, wrap each older handler's `JSON.parse` in try-catch. For example, `stream_created` (line ~57):

Before:
```typescript
es.addEventListener("stream_created", (e: MessageEvent) => {
  const stream = JSON.parse(e.data);
  addOrUpdateStream(stream);
});
```

After:
```typescript
es.addEventListener("stream_created", (e: MessageEvent) => {
  try {
    const stream = JSON.parse(e.data);
    addOrUpdateStream(stream);
  } catch (err) {
    console.error("failed to parse stream_created event:", err);
  }
});
```

Apply the same pattern to: `stream_updated`, `metrics_updated`, `forwarder_race_assigned`, `log_entry`.

**Step 6: Run frontend tests**

Run: `cd apps/server-ui && npm test`
Expected: All tests pass. If any SSE test expectations break due to the type changes, update the test mocks.

Run: `cd apps/shared-ui && npm test`
Expected: All pass.

**Step 7: Commit**

```bash
git add apps/server-ui/src/lib/api.ts apps/server-ui/src/lib/sse.ts apps/server-ui/src/routes/+page.svelte apps/server-ui/src/lib/sse.test.ts
git commit -m "fix(server-ui): fix TS type mismatch, button state, SSE resilience

- Add ReaderControlHttpResponse matching actual HTTP response shape
- Use string literal unions for state/mode fields
- Fix Reconnect button disabled when forwarder offline
- Backfill try-catch to all SSE event handlers"
```

---

### Task 5: Comments & docs (Phase 5)

**Files:**
- Modify: `services/server/src/http/reader_control.rs:13-16` (timeout comment)
- Modify: `docs/plans/2026-03-08-server-reader-control-design.md:65` (endpoint name)

**Step 1: Fix `READER_CONTROL_TIMEOUT` doc comment (issue 3)**

In `services/server/src/http/reader_control.rs`, replace the doc comment above `READER_CONTROL_TIMEOUT` with:
```rust
/// Timeout for reader control request/response round trips.
///
/// ClearRecords, StartDownload, and SyncClock are fire-and-forget or
/// background tasks on the forwarder, so this timeout covers only
/// short command-response cycles (GetInfo, SetReadMode, etc.).
```

**Step 2: Fix design doc endpoint name (issue 13)**

In `docs/plans/2026-03-08-server-reader-control-design.md:65`, change:
```
| POST | `/download-reads` | StartDownload (fire-and-forget, returns 202) |
```
to:
```
| POST | `/start-download` | StartDownload (fire-and-forget, returns 202) |
```

**Step 3: Improve `send_fire_and_forget` doc comment (issue 5c)**

In `services/server/src/http/reader_control.rs`, update the doc comment on `send_fire_and_forget`:
```rust
/// Send a fire-and-forget reader control command.
///
/// Returns 202 if the command was queued, 404 if the forwarder is not
/// connected, or 504 if the command queue is saturated.
```

**Step 4: Commit**

```bash
git add services/server/src/http/reader_control.rs docs/plans/2026-03-08-server-reader-control-design.md
git commit -m "docs: fix inaccurate comments and design doc endpoint name"
```

---

### Task 6: Integration tests (Phase 6)

**Files:**
- Create: `services/server/tests/reader_control_proxy.rs`

This file follows the pattern established in `services/server/tests/forwarder_config_proxy.rs`. Use the same `insert_token` and `make_server` helpers, `MockWsClient`, and `testcontainers::runners::AsyncRunner` setup.

**Step 1: Create test file with shared helpers**

Create `services/server/tests/reader_control_proxy.rs`:

```rust
//! Integration tests for reader control HTTP→WS proxy.

use rt_protocol::WsMessage;
use rt_test_utils::MockWsClient;
use server::AppState;
use sha2::{Digest, Sha256};
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

async fn insert_token(pool: &sqlx::PgPool, device_id: &str, device_type: &str, raw_token: &[u8]) {
    let hash = Sha256::digest(raw_token);
    sqlx::query!(
        "INSERT INTO device_tokens (token_hash, device_type, device_id) VALUES ($1, $2, $3)",
        hash.as_slice(),
        device_type,
        device_id
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn make_server(pool: sqlx::PgPool) -> std::net::SocketAddr {
    let app_state = AppState::new(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, server::build_router(app_state, None))
            .await
            .unwrap();
    });
    addr
}

async fn setup_forwarder(
    addr: std::net::SocketAddr,
    device_id: &str,
    token: &[u8],
    reader_ips: Vec<String>,
) -> MockWsClient {
    let mut fwd = MockWsClient::connect_with_token(
        &format!("ws://{}/ws/forwarder", addr),
        token,
    )
    .await;
    fwd.send_message(&WsMessage::ForwarderHello(rt_protocol::ForwarderHello {
        device_id: device_id.to_owned(),
        version: "test".to_owned(),
        reader_ips,
    }))
    .await;
    // Consume initial heartbeat
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        fwd.recv_message(),
    )
    .await;
    fwd
}
```

**Step 2: Test — happy path round trip**

```rust
#[tokio::test]
async fn reader_control_returns_200_on_success() {
    let container = Postgres::default().start().await.unwrap();
    let pool = sqlx::PgPool::connect(&format!(
        "postgres://postgres:postgres@127.0.0.1:{}/postgres",
        container.get_host_port_ipv4(5432).await.unwrap()
    ))
    .await
    .unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();

    let addr = make_server(pool.clone()).await;
    let token = b"rc-test-token-1";
    let device_id = "rc-fwd-1";
    let reader_ip = "10.20.0.1:10000";
    insert_token(&pool, device_id, "forwarder", token).await;
    let mut fwd = setup_forwarder(addr, device_id, token, vec![reader_ip.to_owned()]).await;

    // Spawn HTTP request
    let http = tokio::spawn({
        let url = format!(
            "http://{}/api/v1/forwarders/{}/readers/{}/info",
            addr, device_id, reader_ip
        );
        async move { reqwest::get(&url).await.unwrap() }
    });

    // Receive the proxied request on the mock forwarder
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
        .await
        .expect("timeout waiting for WS message")
        .expect("WS recv failed");

    let WsMessage::ReaderControlRequest(req) = msg else {
        panic!("expected ReaderControlRequest, got {:?}", msg);
    };
    assert_eq!(req.reader_ip, reader_ip);
    assert_eq!(req.action, rt_protocol::ReaderControlAction::GetInfo);

    // Reply with success
    fwd.send_message(&WsMessage::ReaderControlResponse(
        rt_protocol::ReaderControlResponse {
            request_id: req.request_id,
            reader_ip: reader_ip.to_owned(),
            success: true,
            error: None,
            reader_info: Some(rt_protocol::ReaderInfo {
                banner: Some("IPICO test".into()),
                ..Default::default()
            }),
        },
    ))
    .await;

    let resp = http.await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);
    assert!(body["reader_info"].is_object());
}
```

Note: `ReaderInfo` needs `Default` — check if it already derives it. If not, the test should construct it fully. The existing round-trip test at `rt-protocol` line ~523 constructs it fully, so follow that pattern if `Default` is not derived.

**Step 3: Test — forwarder returns error**

```rust
#[tokio::test]
async fn reader_control_returns_502_on_forwarder_error() {
    let container = Postgres::default().start().await.unwrap();
    let pool = /* same setup */ ;
    let addr = make_server(pool.clone()).await;
    let token = b"rc-test-token-2";
    let device_id = "rc-fwd-2";
    let reader_ip = "10.20.0.2:10000";
    insert_token(&pool, device_id, "forwarder", token).await;
    let mut fwd = setup_forwarder(addr, device_id, token, vec![reader_ip.to_owned()]).await;

    let http = tokio::spawn({
        let url = format!(
            "http://{}/api/v1/forwarders/{}/readers/{}/info",
            addr, device_id, reader_ip
        );
        async move { reqwest::get(&url).await.unwrap() }
    });

    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
        .await.unwrap().unwrap();
    let WsMessage::ReaderControlRequest(req) = msg else { panic!("wrong msg") };

    fwd.send_message(&WsMessage::ReaderControlResponse(
        rt_protocol::ReaderControlResponse {
            request_id: req.request_id,
            reader_ip: reader_ip.to_owned(),
            success: false,
            error: Some("reader not connected".into()),
            reader_info: None,
        },
    )).await;

    let resp = http.await.unwrap();
    assert_eq!(resp.status(), 502);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "READER_CONTROL_ERROR");
}
```

**Step 4: Test — forwarder disconnects before reply**

```rust
#[tokio::test]
async fn reader_control_returns_502_on_forwarder_disconnect() {
    // Same setup...
    let token = b"rc-test-token-3";
    let device_id = "rc-fwd-3";
    let reader_ip = "10.20.0.3:10000";
    // ... insert_token, setup_forwarder ...

    let http = tokio::spawn(/* GET .../info */);

    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
        .await.unwrap().unwrap();
    assert!(matches!(msg, WsMessage::ReaderControlRequest(_)));

    // Disconnect without replying
    fwd.close().await;

    let resp = http.await.unwrap();
    assert_eq!(resp.status(), 502);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "FORWARDER_DISCONNECTED");
}
```

**Step 5: Test — fire-and-forget returns 202**

```rust
#[tokio::test]
async fn fire_and_forget_returns_202() {
    // Same setup...
    let token = b"rc-test-token-4";
    let device_id = "rc-fwd-4";
    let reader_ip = "10.20.0.4:10000";
    // ... insert_token, setup_forwarder ...

    let client = reqwest::Client::new();
    let resp = client
        .post(format!(
            "http://{}/api/v1/forwarders/{}/readers/{}/clear-records",
            addr, device_id, reader_ip
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);

    // Verify the command actually reached the forwarder
    let msg = tokio::time::timeout(std::time::Duration::from_secs(5), fwd.recv_message())
        .await.unwrap().unwrap();
    let WsMessage::ReaderControlRequest(req) = msg else { panic!("wrong msg") };
    assert_eq!(req.action, rt_protocol::ReaderControlAction::ClearRecords);
}
```

**Step 6: Test — reader state cache cleanup on disconnect**

```rust
#[tokio::test]
async fn reader_states_cleaned_up_on_forwarder_disconnect() {
    // Setup with forwarder connected...
    let token = b"rc-test-token-5";
    let device_id = "rc-fwd-5";
    let reader_ip = "10.20.0.5:10000";
    // ... insert_token, setup_forwarder ...

    // Send a ReaderInfoUpdate to populate the cache
    fwd.send_message(&WsMessage::ReaderInfoUpdate(rt_protocol::ReaderInfoUpdate {
        reader_ip: reader_ip.to_owned(),
        state: rt_protocol::ReaderConnectionState::Connected,
        reader_info: None,
    })).await;

    // Give the server a moment to process
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify cache is populated
    let resp = reqwest::get(format!("http://{}/api/v1/reader-states", addr))
        .await.unwrap();
    let states: Vec<serde_json::Value> = resp.json().await.unwrap();
    assert!(!states.is_empty(), "expected reader state in cache");

    // Disconnect forwarder
    fwd.close().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Verify cache is cleaned up (or state changed to disconnected)
    let resp = reqwest::get(format!("http://{}/api/v1/reader-states", addr))
        .await.unwrap();
    let states: Vec<serde_json::Value> = resp.json().await.unwrap();
    // After disconnect, entries should be removed from cache
    assert!(states.is_empty() || states.iter().all(|s| s["state"] == "disconnected"));
}
```

**Step 7: Test — forwarder not connected returns 404**

```rust
#[tokio::test]
async fn reader_control_returns_404_when_forwarder_not_connected() {
    let container = Postgres::default().start().await.unwrap();
    let pool = /* setup */ ;
    let addr = make_server(pool).await;

    let resp = reqwest::get(format!(
        "http://{}/api/v1/forwarders/nonexistent/readers/10.0.0.1:10000/info",
        addr
    ))
    .await.unwrap();
    assert_eq!(resp.status(), 404);
}
```

**Step 8: Test — invalid reader_ip returns 400**

```rust
#[tokio::test]
async fn reader_control_returns_400_for_invalid_reader_ip() {
    let container = Postgres::default().start().await.unwrap();
    let pool = /* setup */ ;
    let addr = make_server(pool.clone()).await;
    let token = b"rc-test-token-7";
    let device_id = "rc-fwd-7";
    insert_token(&pool, device_id, "forwarder", token).await;
    let _fwd = setup_forwarder(addr, device_id, token, vec!["10.20.0.7:10000".to_owned()]).await;

    // No colon in reader_ip
    let resp = reqwest::get(format!(
        "http://{}/api/v1/forwarders/{}/readers/bad-ip/info",
        addr, device_id
    ))
    .await.unwrap();
    assert_eq!(resp.status(), 400);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "INVALID_READER_IP");
}
```

**Step 9: Run tests**

Run: `cargo test -p server --test reader_control_proxy -- --test-threads=4`
Expected: All 7 tests pass.

**Step 10: Commit**

```bash
git add services/server/tests/reader_control_proxy.rs
git commit -m "test(server): add reader control proxy integration tests

Cover happy path, error forwarding, disconnect, fire-and-forget,
state cache cleanup, 404 not connected, and 400 invalid reader_ip."
```

---

### Post-implementation checklist

After all 6 tasks are complete:

1. Run full test suite: `cargo test --workspace && cd apps/server-ui && npm test && cd ../shared-ui && npm test`
2. Verify no clippy warnings: `cargo clippy --workspace`
3. Verify formatting: `cargo fmt --check`
4. Review the full diff: `git diff master...HEAD --stat`
