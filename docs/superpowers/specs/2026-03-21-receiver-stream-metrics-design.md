# Receiver Stream Metrics Design

## Goal

Add server-sourced stream metrics to the receiver's expanded stream row, matching the metrics shown on the server dashboard UI. Metrics are fetched via HTTP on initial load and kept up-to-date via the server's SSE stream.

## Current State

The receiver's expanded stream row shows: Reader IP, Forwarder, Epoch (+name), Reads (total + epoch), and subscription/epoch controls.

The server dashboard shows per-stream metrics in two groups (lifetime and current epoch) with fields like raw count, dedup count, retransmit count, lag, unique chips, last read time, and time since last read. These come from `GET /api/v1/streams/{id}/metrics` and real-time `metrics_updated` SSE events.

The receiver has no access to these metrics today.

## Design

### Data Flow

1. **SSE subscription**: The receiver's Rust backend subscribes to the server's SSE endpoint (`/api/v1/sse`) when the WebSocket connection is established. It listens for `metrics_updated` events, parses them, and forwards them to the frontend via a new Tauri native event `stream_metrics_updated`.
2. **Initial fetch**: On connection (and on resync), the backend fetches `GET /api/v1/streams/{id}/metrics` for each subscribed stream (with a concurrency limit of 4 to avoid burst-loading the server) and emits the results as `stream_metrics_updated` events to the frontend.
3. **Frontend store**: A new reactive map in the store (`streamMetrics: Map<string, StreamMetrics>`) keyed by stream key (`forwarder_id/reader_ip`) holds the latest metrics per stream.
4. **SSE listener**: `sse.ts` registers a handler for the `stream_metrics_updated` Tauri event that updates the store map.

### Stream ID Resolution

The server's SSE `metrics_updated` events and HTTP metrics endpoint use `stream_id` (UUID), but the receiver indexes streams by `forwarder_id/reader_ip`. The receiver already fetches the stream list from `GET /api/v1/streams`, which returns each stream's `id`, `forwarder_id`, and `reader_ip`. The Rust backend maintains an in-memory map of `stream_id → (forwarder_id, reader_ip)` populated from the streams list response. When an SSE `metrics_updated` event arrives, the backend resolves the UUID to the stream key before emitting the Tauri event. Unknown UUIDs (e.g., streams the receiver isn't subscribed to) are silently dropped.

### SSE Connection Lifecycle

- The SSE connection is established when the WebSocket connects and torn down when it disconnects.
- The SSE endpoint does not require authentication (it is a read-only dashboard feed with no auth middleware).
- On SSE connection drop (while WebSocket is still up), the client reconnects with exponential backoff (1s, 2s, 4s, capped at 30s). On reconnect, a full metrics re-fetch is triggered to cover any missed events.
- If the server's broadcast channel lags (overflow), the SSE stream errors out. The reconnect-and-refetch strategy handles this case.

### Field Name Mapping

The server's SSE payload and HTTP response use `lag_ms` and `epoch_lag_ms`. The receiver's Rust backend normalizes these to `lag` and `epoch_lag` (matching the server-ui's convention) before emitting the Tauri event. This keeps the frontend type consistent with the server-ui's `StreamMetrics` interface.

### StreamMetrics Type (TypeScript)

```typescript
interface StreamMetrics {
  raw_count: number;
  dedup_count: number;
  retransmit_count: number;
  lag: number | null;         // milliseconds since last canonical event, null if no events
  epoch_raw_count: number;
  epoch_dedup_count: number;
  epoch_retransmit_count: number;
  unique_chips: number;
  epoch_last_received_at: string | null;  // RFC 3339 timestamp
  epoch_lag: number | null;   // milliseconds, null if no events in epoch
}
```

Fields intentionally excluded: `last_tag_id` and `last_reader_timestamp` (available in the server payload but not displayed in either UI), `backlog` (hardcoded to 0).

### Expanded Row Layout

The existing info grid (Reader IP, Forwarder, Epoch) remains at the top. Below it, two new sections are added before the controls row.

**Lifetime Metrics** (2-column grid, matching existing style):

| Label | Source field | Help text (title attribute) |
|---|---|---|
| Raw count | `raw_count` | Total frames received including retransmits |
| Dedup count | `dedup_count` | Unique frames after deduplication |
| Retransmit | `retransmit_count` | Duplicate frames that matched existing events |
| Lag | `lag` | Time since the last unique frame was received |

**Current Epoch** (2-column grid):

| Label | Source field | Help text (title attribute) |
|---|---|---|
| Raw (epoch) | `epoch_raw_count` | Frames received in the current epoch |
| Dedup (epoch) | `epoch_dedup_count` | Unique frames in the current epoch |
| Retransmit (epoch) | `epoch_retransmit_count` | Duplicate frames in the current epoch |
| Unique chips | `unique_chips` | Distinct chip IDs detected in the current epoch |
| Last read | `epoch_last_received_at` | Timestamp of the last unique frame in the current epoch |
| Time since last read | computed | Live-updating elapsed time since last unique frame |

The existing "Reads (total + epoch)" line in the current expanded row is removed since that information is now covered by the raw/dedup counts. Note: the current "Reads" values come from the receiver's local in-memory counters, while the new metrics come from the server. When metrics are unavailable (server unreachable), the "Metrics unavailable" placeholder is shown — the local read counts in the collapsed row remain visible as a fallback.

### Help Text

Help text is rendered as `title` attributes on the `<dt>` (or label) elements. This gives native browser tooltips on hover — lightweight and consistent with the compact expanded-row style.

### Formatting

- **Lag**: `null` → "N/A (no events yet)", `< 1000ms` → "{lag} ms", `>= 1000ms` → "{lag/1000} s" (matches server UI `formatLag()`)
- **Time since last read**: live-updating via `setInterval(1000)`, formatted as "< 1s" / "Xs" / "Xm Ys" / "Xh Ym Zs" (matches server UI `formatDuration()`)
- **Last read**: `toLocaleString()` of the RFC 3339 timestamp, or "N/A (no events in epoch)" if null
- **Counts**: `.toLocaleString()` for thousands separators

### Metrics Unavailable State

When metrics haven't been fetched yet (e.g., server unreachable), the metrics sections show a single "Metrics unavailable" placeholder instead of empty grids.

### What Changes Where

**Rust backend (`services/receiver/src/`)**:
- New SSE client module that connects to the server's `/api/v1/sse` endpoint
- Metrics fetch function that calls `GET /api/v1/streams/{id}/metrics` for subscribed streams
- New `ReceiverUiEvent` variant for `StreamMetricsUpdated`
- SSE connection lifecycle tied to the WebSocket connection state

**Tauri bridge (`apps/receiver-ui/src-tauri/src/main.rs`)**:
- Forward the new `StreamMetricsUpdated` event through the existing event bridge

**Frontend store (`apps/receiver-ui/src/lib/`)**:
- `store.svelte.ts`: new `streamMetrics` reactive map, update function
- `sse.ts`: listener for `stream_metrics_updated` Tauri event
- `api.ts`: `StreamMetrics` TypeScript interface

**UI (`apps/receiver-ui/src/lib/components/StreamsTab.svelte`)**:
- Expanded row: add lifetime and current epoch metrics sections
- `setInterval` for live time-since-last-read counter, setup/cleanup via Svelte 5 `$effect` return function (matching server-ui pattern)
- Remove the existing "Reads" line from the expanded row (superseded by new metrics)

## Out of Scope

- Backlog metric (hardcoded to 0 on the server, not useful)
- Promoting metrics to the collapsed row
- Offline/local metric computation
- Adding help text to the server UI (separate concern)
