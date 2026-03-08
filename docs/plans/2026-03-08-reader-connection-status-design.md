# Reader Connection Status Design

## Problem

When a forwarder is connected to the server but disconnected from a reader, the server shows the stream as "online." There is no visibility into whether the forwarder actually has a live TCP connection to the reader. Operators and timing software cannot distinguish "everything working" from "forwarder up but reader unreachable."

## Decision

Add a `reader_connected` status independent of the existing `online` status. `online` remains "forwarder WS session is alive." `reader_connected` means "forwarder has an active TCP connection to the reader."

## Protocol Changes (rt-protocol)

New forwarder-to-server variant:

```rust
ReaderStatusUpdate {
    reader_ip: String,
    connected: bool,
}
```

Sent on each reader TCP state transition (Connected / Disconnected) and as an initial burst after `ForwarderHello` on connect.

New server-to-receiver variant:

```rust
ReaderStatusChanged {
    stream_id: Uuid,
    reader_ip: String,
    connected: bool,
}
```

Two separate variants because the forwarder doesn't know `stream_id`; the server resolves it.

## Server Changes

### Database

Add column to `streams` table:

```sql
ALTER TABLE streams ADD COLUMN reader_connected BOOLEAN NOT NULL DEFAULT false;
```

When the forwarder WS disconnects, both `online` and `reader_connected` are set to `false`.

### HTTP API

`GET /api/v1/streams` response gains `reader_connected: bool` per stream.

### SSE Events

- `StreamCreated` gains `reader_connected: bool`
- `StreamUpdated` gains `reader_connected: Option<bool>` (present only on change)

### WS Fanout

On receiving `ReaderStatusUpdate`, the server updates the DB, emits `StreamUpdated` on SSE, and forwards `ReaderStatusChanged` to connected receivers subscribed to that stream.

## Forwarder Changes

In `run_reader` (main.rs), after each `ReaderConnectionState` transition to `Connected` or `Disconnected`, send `ReaderStatusUpdate` over the uplink WS if connected.

On initial uplink connect, after sending `ForwarderHello`, send one `ReaderStatusUpdate` per reader with its current state.

If the uplink is not connected when a reader state changes, no message is sent. The state is sent when the uplink reconnects (initial burst after hello).

## Receiver Changes

Receives `ReaderStatusChanged` via WS. Stores `reader_connected` per stream in local state. Surfaces a warning when `reader_connected` flips to `false` for a previously-connected stream. Existing reads remain valid and are not discarded.

## Dashboard Changes

Consumes `reader_connected` from the streams API and SSE events. Displays both statuses per stream (e.g., "Online / Reader disconnected").

## Status Matrix

| online | reader_connected | Meaning |
|--------|-----------------|---------|
| false  | false           | Forwarder offline |
| true   | false           | Forwarder connected, reader unreachable |
| true   | true            | Fully operational |
| false  | true            | Invalid (cleared on WS disconnect) |
