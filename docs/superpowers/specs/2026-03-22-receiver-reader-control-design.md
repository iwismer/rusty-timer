# Receiver Reader Control via WS Proxy

## Summary

Add reader control (clock sync, read mode, TTO, recording, download, etc.) to the receiver, proxied through the server over existing WS connections. Also tunnel `ReaderInfoUpdate` and `ReaderDownloadProgress` events to receivers for real-time feedback. Add reader control UI to both the receiver streams tab and the server stream detail page via a shared component.

## Motivation

Reader control is currently only accessible via HTTP endpoints on the server. Race operators using the receiver desktop app must switch to the server dashboard to manage readers. Proxying these commands over the existing receiver WS connection eliminates the need for separate HTTP auth and keeps the workflow within the receiver UI.

## Approach

Use a single `ReceiverProxyReaderControlRequest/Response` pair wrapping the existing `ReaderControlAction` enum, rather than 10 dedicated variant pairs. This keeps the protocol lean (4 new WsMessage variants total) while providing full parity with all 10 reader control actions.

For real-time feedback, tunnel `ReaderInfoUpdate` and `ReaderDownloadProgress` from the forwarder to all subscribed receivers via the existing sentinel-in-broadcast-channel pattern used by `ReaderStatusChanged`.

## Scope

### In scope
- All 10 `ReaderControlAction` variants: `GetInfo`, `SyncClock`, `SetReadMode`, `SetTto`, `SetRecording`, `ClearRecords`, `StartDownload`, `StopDownload`, `Refresh`, `Reconnect`
- WS proxy: receiver -> server -> forwarder -> reader, response back
- Tunneling `ReaderInfoUpdate` and `ReaderDownloadProgress` to receivers
- Shared `ReaderControlPanel` UI component
- Reader control card on server stream detail page (above epoch mapping card)
- Reader control section in receiver streams tab expanded row

### Out of scope
- New authentication mechanisms
- Changes to the forwarder's reader control implementation
- Changes to existing HTTP reader control endpoints

---

## Protocol Layer (`crates/rt-protocol`)

### New structs

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

### New WsMessage variants

```rust
ReceiverProxyReaderControlRequest(ReceiverProxyReaderControlRequest)
ReceiverProxyReaderControlResponse(ReceiverProxyReaderControlResponse)
ReceiverReaderInfoUpdate(ReceiverReaderInfoUpdate)
ReceiverReaderDownloadProgress(ReceiverReaderDownloadProgress)
```

### New sentinel constants

```rust
pub const READER_INFO_UPDATED_READ_TYPE: &str = "__reader_info_updated";
pub const READER_DOWNLOAD_PROGRESS_READ_TYPE: &str = "__reader_download_progress";
```

---

## Server Layer (`services/server`)

### `ws_receiver.rs` - Proxy handler

Add a match arm for `ReceiverProxyReaderControlRequest` in the incoming message handler. Spawn into the existing `pending_proxy_replies: JoinSet<WsMessage>`:

```
proxy_reader_control_reply(state, req) -> WsMessage::ReceiverProxyReaderControlResponse
```

The async function:
1. Looks up `state.forwarder_command_senders` for `req.forwarder_id`
2. For request/response actions (`GetInfo`, `SyncClock`, `SetReadMode`, `SetTto`, `SetRecording`, `StopDownload`, `Refresh`, `Reconnect`): creates a oneshot, sends `ForwarderCommand::ReaderControl`, awaits reply with timeout
3. For fire-and-forget actions (`ClearRecords`, `StartDownload`): sends `ForwarderCommand::ReaderControlFireAndForget`, returns `ok: true` immediately
4. Maps `ForwarderProxyReply::Response(resp)` to the response struct, handling `Timeout` and `InternalError` as `ok: false`

### `ws_receiver.rs` - Sentinel intercepts

Extend the sentinel interception block to handle:
- `__reader_info_updated` -> forward JSON to receiver socket
- `__reader_download_progress` -> forward JSON to receiver socket

Same pattern as `__reader_status_changed`.

### `ws_forwarder.rs` - Sentinel broadcasts

After the existing `ReaderInfoUpdate` handler (which writes to `state.reader_states` and sends `DashboardEvent::ReaderInfoUpdated`), add:
1. Look up stream(s) for this forwarder + reader_ip
2. Build `ReceiverReaderInfoUpdate` with `stream_id`, serialize to JSON
3. Send as sentinel `ReadEvent` with `read_type = "__reader_info_updated"` into the stream's broadcast channel

After the existing `ReaderDownloadProgress` handler (which sends `DashboardEvent::ReaderDownloadProgress`), add:
1. Look up stream(s) for this forwarder + reader_ip
2. Build `ReceiverReaderDownloadProgress` with `stream_id`, serialize to JSON
3. Send as sentinel `ReadEvent` with `read_type = "__reader_download_progress"` into the stream's broadcast channel

---

## Receiver Backend (`services/receiver`)

### `session.rs` - WsCommand

Add `WsMessage::ReceiverProxyReaderControlRequest` to `WsCommand::new()`'s match arm to extract `request_id`.

### `session.rs` - Incoming message handling

Add match arms:
- `ReceiverProxyReaderControlResponse`: route through existing `pending_requests` HashMap via `request_id` (same as all other proxy responses)
- `ReceiverReaderInfoUpdate`: emit `ReceiverUiEvent::ReaderInfoUpdated`
- `ReceiverReaderDownloadProgress`: emit `ReceiverUiEvent::ReaderDownloadProgress`

### New `ReceiverUiEvent` variants

```rust
ReaderInfoUpdated {
    stream_id: Uuid,
    reader_ip: String,
    state: ReaderConnectionState,
    reader_info: Option<ReaderInfo>,
}

ReaderDownloadProgress {
    stream_id: Uuid,
    reader_ip: String,
    state: DownloadState,
    reads_received: u32,
    progress: u64,
    total: u64,
    error: Option<String>,
}
```

### `control_api.rs` - New Tauri commands

10 new commands, one per `ReaderControlAction`:
- `reader_get_info(forwarder_id, reader_ip)`
- `reader_sync_clock(forwarder_id, reader_ip)`
- `reader_set_read_mode(forwarder_id, reader_ip, mode, timeout)`
- `reader_set_tto(forwarder_id, reader_ip, enabled)`
- `reader_set_recording(forwarder_id, reader_ip, enabled)`
- `reader_clear_records(forwarder_id, reader_ip)`
- `reader_start_download(forwarder_id, reader_ip)`
- `reader_stop_download(forwarder_id, reader_ip)`
- `reader_refresh(forwarder_id, reader_ip)`
- `reader_reconnect(forwarder_id, reader_ip)`

Each builds a `ReceiverProxyReaderControlRequest`, sends via `WsCommand` through `ws_cmd_tx`, awaits the oneshot reply with a 15s timeout.

---

## UI Layer

### Shared component (`apps/shared-ui`)

Extract a `ReaderControlPanel.svelte` component from the existing reader control UI on the server-ui main page. Props:

- `readerIp: string`
- `readerState: ReaderConnectionState`
- `readerInfo: ReaderInfo | null`
- `downloadProgress: DownloadProgressData | null`
- `onAction: (action: string, params?: Record<string, unknown>) => Promise<ActionResult>`
- `disabled: boolean` (e.g., when forwarder is offline)

The component renders:
- Reader info display (firmware, read mode, TTO, recording status, record count, clock offset)
- Action buttons: Sync Clock, Refresh, Reconnect
- Read Mode select + apply
- TTO toggle, Recording toggle
- Clear Records, Start Download, Stop Download buttons
- Download progress bar (when active)
- Inline feedback (success/error messages per action, with auto-dismiss)

Reuses existing helpers from `@rusty-timer/shared-ui/lib/reader-view-model` and `read-mode-form`.

### Server-UI stream detail page (`apps/server-ui/src/routes/streams/[streamId]/+page.svelte`)

Add `<Card title="Reader Control">` above the Epoch Race Mapping card containing `<ReaderControlPanel>`. Wire `onAction` to the existing HTTP API functions in `server-ui/api.ts`. Reader state comes from `readerStatesStore` and `downloadProgressStore`.

### Receiver-UI streams tab (`apps/receiver-ui/src/lib/components/StreamsTab.svelte`)

Add `<ReaderControlPanel>` inside the expanded row, below the metrics grid and above the action row. Wire `onAction` to the new Tauri commands in `receiver-ui/api.ts`. Reader state comes from new Tauri event listeners for `ReaderInfoUpdated` and `ReaderDownloadProgress`.

New functions in `apps/receiver-ui/src/lib/api.ts`:
- `readerGetInfo(forwarderId, readerIp)`
- `readerSyncClock(forwarderId, readerIp)`
- `readerSetReadMode(forwarderId, readerIp, mode, timeout)`
- `readerSetTto(forwarderId, readerIp, enabled)`
- `readerSetRecording(forwarderId, readerIp, enabled)`
- `readerClearRecords(forwarderId, readerIp)`
- `readerStartDownload(forwarderId, readerIp)`
- `readerStopDownload(forwarderId, readerIp)`
- `readerRefresh(forwarderId, readerIp)`
- `readerReconnect(forwarderId, readerIp)`

---

## Error Handling

| Scenario | Behavior |
|---|---|
| Forwarder offline | Server proxy returns `ok: false, error: "forwarder not connected"` |
| Reader disconnected | Forwarder returns `success: false, error: "reader not found"`, proxied back |
| Timeout (server <-> forwarder) | 10s timeout on oneshot, returns `ok: false, error: "timeout"` |
| Timeout (receiver <-> server) | 15s timeout on Tauri command to account for extra hop |
| Fire-and-forget dispatch failure | Returns `ok: false` with error; progress tracked via tunneled events |
| WS disconnected mid-request | Receiver session reconnect logic; pending requests get dropped, UI shows connection error |

---

## Data Flow

### Reader control (receiver-initiated)
```
Receiver UI -> Tauri command -> WsCommand(ReceiverProxyReaderControlRequest)
  -> receiver session.rs: send over WS
  -> server ws_receiver.rs: spawn proxy_reader_control_reply()
    -> ForwarderCommand::ReaderControl via forwarder_command_senders
    -> server ws_forwarder.rs: send ReaderControlRequest over forwarder WS
    -> forwarder uplink_task.rs: handle_reader_control_message()
      -> ControlClient -> IPICO reader TCP
    -> forwarder: send ReaderControlResponse back
  -> server ws_forwarder.rs: resolve pending oneshot
  -> server ws_receiver.rs: JoinSet yields ReceiverProxyReaderControlResponse
  -> receiver session.rs: resolve pending_requests oneshot
  -> Tauri command returns -> UI updates
```

### Reader info/progress broadcast
```
Forwarder: ReaderInfoUpdate / ReaderDownloadProgress over WS
  -> server ws_forwarder.rs: write cache + dashboard event (existing)
  -> server ws_forwarder.rs: build ReceiverReaderInfoUpdate, serialize as sentinel ReadEvent
  -> broadcast channel -> all subscribed receiver sessions
  -> server ws_receiver.rs: sentinel intercept, forward JSON to receiver socket
  -> receiver session.rs: emit ReceiverUiEvent
  -> Tauri event -> UI updates
```

---

## Testing

### Protocol (`rt-protocol`)
- Serde round-trip for all 4 new structs
- WsMessage serialization produces correct `kind` tags

### Server proxy handler
- Proxy request with forwarder online -> response flows back
- Proxy request with forwarder offline -> `ok: false` error
- Fire-and-forget actions return `ok: true` immediately
- Timeout handling

### Server sentinel tunneling
- `ReaderInfoUpdate` from forwarder -> sentinel -> reaches receiver WS
- `ReaderDownloadProgress` from forwarder -> sentinel -> reaches receiver WS

### Receiver session
- `WsCommand::new()` accepts `ReceiverProxyReaderControlRequest`
- Incoming `ReceiverProxyReaderControlResponse` resolves pending request
- Incoming `ReceiverReaderInfoUpdate` emits correct event
- Incoming `ReceiverReaderDownloadProgress` emits correct event

### UI
- Manual testing for the shared `ReaderControlPanel` component in both apps
