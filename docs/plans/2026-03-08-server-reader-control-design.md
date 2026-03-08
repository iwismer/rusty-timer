# Server-Side Reader Control

## Summary

Add remote reader control to the server so the dashboard can manage IPICO readers without direct network access to the forwarder. Commands are proxied through the existing forwarder WebSocket connection using the same request/response pattern as the config proxy.

## Protocol

### New WsMessage variants (rt-protocol)

**ReaderControlRequest** (Server -> Forwarder):
- `request_id: String`
- `reader_ip: String`
- `action: ReaderControlAction`

**ReaderControlAction** enum:
- `GetInfo`
- `SyncClock`
- `SetReadMode { mode: String, timeout: u8 }`
- `SetTto { enabled: bool }`
- `SetRecording { enabled: bool }`
- `ClearRecords`
- `StartDownload`
- `StopDownload`
- `Refresh`
- `Reconnect`

**ReaderControlResponse** (Forwarder -> Server):
- `request_id: String`
- `reader_ip: String`
- `success: bool`
- `error: Option<String>`
- `reader_info: Option<ReaderInfo>`

**ReaderInfoUpdate** (Forwarder -> Server, unsolicited):
- `reader_ip: String`
- `state: String` ("connected", "connecting", "disconnected")
- `reader_info: Option<ReaderInfo>` (None when disconnected)

**ReaderDownloadProgress** (Forwarder -> Server, unsolicited):
- `reader_ip: String`
- `state: String` ("starting", "downloading", "complete", "error")
- `reads_received: u32`
- `progress: u64`
- `total: u64`
- `error: Option<String>`

**ReaderInfo** (shared struct in rt-protocol):
Mirrors the forwarder's existing `ReaderInfo`: banner, hardware (fw_version, hw_code, reader_id), config (mode, timeout), tto_enabled, clock (reader_clock, drift_ms), estimated_stored_reads, recording, connect_failures.

## Server

### HTTP Endpoints

All under `/api/v1/forwarders/{forwarder_id}/readers/{reader_ip}/`:

| Method | Path | Action |
|--------|------|--------|
| GET | `/info` | GetInfo |
| POST | `/sync-clock` | SyncClock |
| PUT | `/read-mode` | SetReadMode |
| PUT | `/tto` | SetTto |
| PUT | `/recording` | SetRecording |
| POST | `/clear-records` | ClearRecords (fire-and-forget, returns 202) |
| POST | `/download-reads` | StartDownload (fire-and-forget, returns 202) |
| POST | `/stop-download` | StopDownload |
| POST | `/refresh` | Refresh |
| POST | `/reconnect` | Reconnect |
| GET | `/reader-states` (top-level) | Returns all cached reader states |

### Command Routing

Same pattern as config proxy:
1. Look up forwarder's `mpsc::Sender<ForwarderCommand>` from `forwarder_command_senders`
2. Create `ForwarderCommand::ReaderControl` with a oneshot reply channel
3. Send through mpsc, await oneshot with 10s timeout
4. Return response as JSON

For `ClearRecords` and `StartDownload`: return 202 immediately, result arrives via unsolicited `ReaderInfoUpdate` / `ReaderDownloadProgress` push.

### New ForwarderCommand Variant

```
ForwarderCommand::ReaderControl {
    request_id: String,
    reader_ip: String,
    action: ReaderControlAction,
    reply: oneshot::Sender<ForwarderProxyReply<ReaderControlResponse>>,
}
```

### Server-Side State

In-memory `HashMap<(forwarder_id, reader_ip), ReaderState>` on `AppState`, populated from `ReaderInfoUpdate` WS messages, cleared on forwarder disconnect.

### New DashboardEvents (SSE)

- `reader_info_updated { forwarder_id, reader_ip, state, reader_info }`
- `reader_download_progress { forwarder_id, reader_ip, state, reads_received, progress, total, error }`

## Forwarder

### WS Command Handler

New match arm in the WS session loop for `ReaderControlRequest`:
1. Look up `ControlClient` by `reader_ip`
2. Match on `action`, delegate to existing `ControlClient` methods
3. Run follow-up `run_status_poll_merge_successes()` where appropriate
4. Send `ReaderControlResponse` with updated `ReaderInfo`

### Unsolicited Pushes

When reader state changes (connect/disconnect/poll), send `ReaderInfoUpdate` upstream in addition to the existing local `ForwarderUiEvent` broadcast. During downloads, forward `DownloadTracker` events as `ReaderDownloadProgress` WS messages.

## Dashboard UI

### Expandable Reader Panel

Each stream card gets a "Details" chevron button. When expanded, shows:

**Info grid** (same layout as forwarder-ui):
- Banner, Firmware, Hardware
- Reader Clock (ticking), Clock Drift
- Local Clock, Last Refresh
- Read Mode (select + timeout input + Apply button)
- TTO state + toggle button

**Action buttons:**
- Sync Clock, Refresh, Start/Stop Recording, Download Reads, Clear Records, Reconnect

**Download progress bar** (same as forwarder-ui).

Controls disabled when reader not connected or forwarder offline.

### State Management

- `readerStateStore` — `Record<string, { forwarder_id, reader_ip, state, reader_info }>` from SSE
- `downloadProgressStore` — `Record<string, ReaderDownloadProgress>` from SSE
- `expandedReader` — local `string | null`
- Per-reader `controlBusy`, `controlFeedback`, `readModeDrafts`

### Shared Code

Port from forwarder-ui to shared-ui (pure functions, ~50 lines):
- `formatReadMode`, `formatTtoState`, `formatClockDrift`
- `readerControlDisabled`, `computeDownloadPercent`
- `read-mode-form.ts` draft helpers
- Reader status cache / ticking clock logic

### Initial Load

New `GET /api/v1/reader-states` endpoint fetched on page load / resync.

## Error Handling

- **Forwarder offline**: Server returns 404 (no mpsc sender). UI disables controls.
- **Reader disconnected**: Forwarder returns `success: false`. UI disables controls based on push state.
- **Forwarder disconnects mid-request**: Server oneshot times out (10s) -> 504. Server clears cached reader state, pushes disconnected state for all readers.
- **Fire-and-forget ops**: Server returns 202. Result arrives via unsolicited push.
- **Forwarder reconnect**: Fresh `ReaderInfoUpdate` messages replace stale cached state.
- **Multiple dashboards**: SSE fanout keeps all in sync. `ControlClient` in-flight lock serializes concurrent requests.
