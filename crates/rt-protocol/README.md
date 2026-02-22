# rt-protocol

WebSocket and HTTP message protocol definitions for the v1 remote forwarding protocol.

## Purpose

Defines the frozen v1 protocol types shared between the forwarder, server, and receiver. All WebSocket messages use a discriminated `kind` tag for serialization via serde. This crate contains no runtime logic -- only data structures and serialization.

## Key types

### Top-level envelope

- **`WsMessage`** -- Tagged enum (`#[serde(tag = "kind")]`) covering all v1 WebSocket message kinds.

### Shared sub-types

- **`ReadEvent`** -- A single chip-read event carried in event batches.
- **`ResumeCursor`** -- Resume position for a (stream, epoch) pair, used by receiver hello.
- **`AckEntry`** -- High-water mark for a (stream, epoch) pair in ack messages.
- **`StreamRef`** -- Immutable stream identity (forwarder_id + reader_ip).

### Forwarder messages

- **`ForwarderHello`** -- Initial handshake from forwarder to server.
- **`ForwarderEventBatch`** -- Batch of read events from forwarder to server.
- **`ForwarderAck`** -- Server acknowledgement of a persisted batch.

### Receiver messages

- **`ReceiverHello`** -- Initial handshake from receiver to server.
- **`ReceiverSubscribe`** -- Mid-session stream subscription request.
- **`ReceiverEventBatch`** -- Batch of read events from server to receiver.
- **`ReceiverAck`** -- Receiver acknowledgement of a delivered batch.

### Bidirectional / server-initiated

- **`Heartbeat`** -- Server heartbeat (30 s interval); carries `session_id` and `device_id`.
- **`ErrorMessage`** -- Protocol error with code, message, and retryable flag.
- **`EpochResetCommand`** -- Server command to reset a stream's epoch.

### Config messages

- **`ConfigGetRequest`** / **`ConfigGetResponse`** -- Remote config retrieval.
- **`ConfigSetRequest`** / **`ConfigSetResponse`** -- Remote config update.
- **`RestartRequest`** / **`RestartResponse`** -- Graceful restart command.

### HTTP API types

- **`StreamInfo`** -- Entry in the `GET /api/v1/streams` response.
- **`StreamMetrics`** -- Response for `GET /api/v1/streams/{id}/metrics`.
- **`StreamPatchRequest`** -- Body for `PATCH /api/v1/streams/{id}`.
- **`ResetEpochResponse`** -- Response for `POST /api/v1/streams/{id}/reset-epoch`.
- **`HttpErrorEnvelope`** -- Standard error envelope for non-2xx responses.

### Error codes

- **`error_codes`** module -- Frozen v1 error code constants (`INVALID_TOKEN`, `SESSION_EXPIRED`, `PROTOCOL_ERROR`, `IDENTITY_MISMATCH`, `INTEGRITY_CONFLICT`, `INTERNAL_ERROR`).
