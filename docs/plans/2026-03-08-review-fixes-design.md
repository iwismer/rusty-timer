# Server Reader Control — PR Review Fixes Design

**Date:** 2026-03-08
**PR:** #147 `feat: add server-side reader control`

## Overview

Address all issues identified in the comprehensive PR review, organized into 6 phases by layer (protocol → server → forwarder → frontend → docs → tests).

## Phase 1: Protocol Enums

Add three enums to `crates/rt-protocol/src/lib.rs`:

- **`ReadMode`** — `Raw`, `Event`, `FirstLastSeen` with `#[serde(rename_all = "snake_case")]` (custom rename `"fsls"` on `FirstLastSeen`)
- **`ReaderConnectionState`** — `Connected`, `Connecting`, `Disconnected`
- **`DownloadState`** — `Downloading`, `Complete`, `Error`, `Idle`

Update structs to use these enums:
- `ReaderControlAction::SetReadMode { mode: ReadMode, timeout: u8 }`
- `Config3Info { mode: Option<ReadMode>, ... }`
- `ReaderInfoUpdate { state: ReaderConnectionState, ... }`
- `ReaderDownloadProgress { state: DownloadState, ... }`
- `CachedReaderState { state: ReaderConnectionState, ... }` (in `services/server/src/state.rs`)

Ripple effects: forwarder's `handle_reader_control_message` switches from string matching to enum matching. Server disconnect cleanup uses `ReaderConnectionState::Disconnected`. Existing serde round-trip tests updated.

## Phase 2: Server Rust Fixes

### 2a. Fix serialization failure sending misleading `Timeout` reply
Add `ForwarderProxyReply::InternalError(String)` variant. In `ws_forwarder.rs`, use it when `serde_json::to_string` fails. Map to HTTP 500 in `reader_control.rs`.

### 2b. Eliminate fire-and-forget warning spam
Add `ForwarderCommand::ReaderControlFireAndForget` variant that sends the command over WS without creating a `pending_reader_controls` entry. No spurious warn, no 10s pending entry leak.

### 2c. Add request logging
`tracing::debug!` in `send_reader_control` and `send_fire_and_forget` for requests. `tracing::warn!` for failures.

### 2d. Validate `reader_ip`
Validation helper checking `<ip>:<port>` format (contains colon, non-empty parts). Return 400 on invalid input.

### 2e. Centralize composite key
Add `pub fn reader_cache_key(forwarder_id: &str, reader_ip: &str) -> String` in `state.rs`. Replace inline `format!` in `ws_forwarder.rs`.

### 2f. Use `#[serde(flatten)]` in `DashboardEvent::ReaderDownloadProgress`
Replace duplicated fields with `forwarder_id: String` + `#[serde(flatten)] progress: rt_protocol::ReaderDownloadProgress`.

## Phase 3: Forwarder Rust Fixes

### 3a. Spawn SyncClock as background task
Follow `ClearRecords`/`StartDownload` pattern: `tokio::spawn`, return success immediately, push result as unsolicited `ReaderInfoUpdate` when complete.

### 3b. Look up actual reader state
In uplink loop's `ForwarderUiEvent::ReaderInfoUpdated` drain, look up real connection state from subsystem status instead of hardcoding `ReaderConnectionState::Connected`.

## Phase 4: Frontend Fixes

### 4a. Fix TS `ReaderControlResponse` type mismatch
Define `ReaderControlHttpResponse { ok: boolean, error: string | null, reader_info: ReaderInfo | null }` matching the actual HTTP response shape. Update `apiFetch` call sites.

### 4b. Fix Reconnect button disabled state
Change `disabled={busy}` to `disabled={busy || !stream.online}`.

### 4c. Backfill try-catch to older SSE handlers
Wrap `JSON.parse(e.data)` in try-catch for `stream_created`, `stream_updated`, `metrics_updated`, `forwarder_race_assigned`, `log_entry`.

### 4d. Use TypeScript string literal union types
- `CachedReaderState.state: "connected" | "connecting" | "disconnected"`
- `ReaderDownloadProgressEvent.state: "downloading" | "complete" | "error" | "idle"`
- `Config3Info.mode: "raw" | "event" | "fsls"`

Remove unnecessary cast in `+page.svelte`'s `readModeDraftValue`.

## Phase 5: Comments & Docs

### 5a. Fix `READER_CONTROL_TIMEOUT` doc comment
Rewrite to reflect that SyncClock, ClearRecords, and StartDownload are all fire-and-forget/background.

### 5b. Fix design doc endpoint name
`/download-reads` → `/start-download` in design doc.

### 5c. Improve `send_fire_and_forget` doc comment
Document all return codes: 202, 404, 504.

## Phase 6: Integration Tests

New file: `services/server/tests/reader_control_proxy.rs` following `forwarder_config_proxy.rs` pattern.

Tests:
1. Happy-path round trip (HTTP → WS → response → HTTP 200)
2. Forwarder returns error → HTTP 502 `READER_CONTROL_ERROR`
3. Forwarder disconnects before reply → HTTP 502 `FORWARDER_DISCONNECTED`
4. Fire-and-forget returns 202 immediately
5. Reader state cache cleanup on forwarder disconnect
6. Forwarder not connected → HTTP 404
7. Invalid `reader_ip` → HTTP 400
