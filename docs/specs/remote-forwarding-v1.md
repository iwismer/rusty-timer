# Remote Forwarding Suite - Protocol Specification v1

## Overview

This document is the frozen v1 specification for the Remote Forwarding Suite.
It covers:

- WebSocket protocol (forwarder and receiver endpoints)
- HTTP API contracts (streams, metrics, exports, epoch reset)
- Export format definitions
- Error codes and envelopes

The authoritative Rust types live in `crates/rt-protocol/src/lib.rs`.
The machine-readable JSON Schema lives in `contracts/ws/v1/messages.schema.json`.
Example JSON messages are in `contracts/ws/v1/examples/`.

---

## Identity Model

| Concept           | Definition                                          |
|-------------------|-----------------------------------------------------|
| Stream identity   | `(forwarder_id, reader_ip)` — immutable             |
| Event identity    | `(forwarder_id, reader_ip, stream_epoch, seq)`      |
| Device identity   | Authoritative from bearer-token claims              |
| Forwarder hello   | `forwarder_id` is advisory; must match claims       |
| Receiver hello    | `receiver_id` is advisory; must match claims        |

If a hello message supplies an ID that does not match the token claims the
server rejects the connection with `IDENTITY_MISMATCH`.

---

## Transport and Authentication

| Parameter       | Value                                                |
|-----------------|------------------------------------------------------|
| Forwarder URL   | `wss://<server>/ws/v1/forwarders`                    |
| Receiver URL    | `wss://<server>/ws/v1/receivers`                     |
| Auth header     | `Authorization: Bearer <device_token>`               |
| Token storage   | Server stores `SHA-256(token)`, never raw token      |
| TLS             | Required; all connections are outbound-only          |
| Revocation      | Blocks new sessions; existing sessions unaffected    |
| Duplicate conn  | First-connection-wins; new connection is rejected    |

---

## WebSocket Session Lifecycle

1. Client opens WS connection with `Authorization: Bearer <token>`.
2. Client immediately sends `forwarder_hello` or `receiver_hello`.
   - Hello messages do NOT include `session_id`.
3. Server validates token and hello, assigns `session_id`.
4. Server sends the initial `heartbeat` carrying both `session_id` and
   `device_id`.
5. Client records `session_id`; echoes it in all subsequent messages that
   carry `session_id`.
6. Client starts its heartbeat timer only AFTER receiving the first server
   heartbeat.
7. Bidirectional heartbeats at 30-second intervals; 90-second timeout
   (3 missed heartbeats) triggers disconnect.

---

## Message Reference

All messages are JSON objects. The top-level `"kind"` field identifies the
message type and acts as the discriminator.

### `forwarder_hello`

Direction: Forwarder -> Server

Sent immediately after WS connection and again after each epoch reset.
The `resume` field implicitly subscribes the forwarder to receive acks for
listed streams.

| Field        | Type              | Notes                                          |
|--------------|-------------------|------------------------------------------------|
| kind         | `"forwarder_hello"` |                                              |
| forwarder_id | string            | Advisory; must match token claims if present   |
| reader_ips   | string[]          | IPs of locally attached readers                |
| resume       | ResumeCursor[]    | Empty array = start fresh; omit = same as []   |

**No `session_id` field.**

```json
{
  "kind": "forwarder_hello",
  "forwarder_id": "fwd-001",
  "reader_ips": ["192.168.1.10"],
  "resume": [
    {"forwarder_id": "fwd-001", "reader_ip": "192.168.1.10", "stream_epoch": 1, "last_seq": 4200}
  ]
}
```

---

### `forwarder_event_batch`

Direction: Forwarder -> Server

| Field      | Type         | Notes                                              |
|------------|--------------|----------------------------------------------------|
| kind       | `"forwarder_event_batch"` |                                       |
| session_id | string       | Echoed from first heartbeat                        |
| batch_id   | string       | Opaque correlation ID — no ack semantic meaning    |
| events     | ReadEvent[]  |                                                    |

`batch_id` is for logging and debugging only. The server ignores it for ack
logic; acks are per `(stream, epoch, last_seq)`.

---

### `forwarder_ack`

Direction: Server -> Forwarder

Sent once per persisted batch. A single message may include multiple entries
when the batch spans epoch boundaries.

| Field      | Type        | Notes                  |
|------------|-------------|------------------------|
| kind       | `"forwarder_ack"` |                  |
| session_id | string      |                        |
| entries    | AckEntry[]  | At least one entry     |

---

### `receiver_hello`

Direction: Receiver -> Server

| Field       | Type            | Notes                                         |
|-------------|-----------------|-----------------------------------------------|
| kind        | `"receiver_hello"` |                                            |
| receiver_id | string          | Advisory; must match token claims if present  |
| resume      | ResumeCursor[]  | Implicitly subscribes to listed streams       |

**No `session_id` field.**

---

### `receiver_subscribe`

Direction: Receiver -> Server

Adds new stream subscriptions mid-session. There is no unsubscribe in v1.

| Field      | Type        | Notes   |
|------------|-------------|---------|
| kind       | `"receiver_subscribe"` | |
| session_id | string      |         |
| streams    | StreamRef[] |         |

---

### `receiver_event_batch`

Direction: Server -> Receiver

| Field      | Type         | Notes   |
|------------|--------------|---------|
| kind       | `"receiver_event_batch"` | |
| session_id | string       |         |
| events     | ReadEvent[]  |         |

---

### `receiver_ack`

Direction: Receiver -> Server

| Field      | Type        | Notes              |
|------------|-------------|--------------------|
| kind       | `"receiver_ack"` |               |
| session_id | string      |                    |
| entries    | AckEntry[]  | At least one entry |

---

### `heartbeat`

Direction: Server -> Client

| Field      | Type         | Notes                                                      |
|------------|--------------|------------------------------------------------------------|
| kind       | `"heartbeat"` |                                                          |
| session_id | string       | Assigned by server; first heartbeat establishes session    |
| device_id  | string       | Resolved from token claims; devices use this to learn ID   |

The initial server heartbeat is structurally identical to ongoing heartbeats.
Clients use `device_id` from the first heartbeat to learn their own identity.

---

### `error`

Direction: Server -> Client

| Field     | Type    | Notes                            |
|-----------|---------|----------------------------------|
| kind      | `"error"` |                                |
| code      | string  | One of the frozen v1 error codes |
| message   | string  | Human-readable description       |
| retryable | boolean | Whether the client should retry  |

#### Frozen v1 Error Codes

| Code               | Retryable | Description                                              |
|--------------------|-----------|----------------------------------------------------------|
| INVALID_TOKEN      | false     | Bearer token is missing, malformed, or revoked           |
| SESSION_EXPIRED    | true      | Session has expired; reconnect and re-hello              |
| PROTOCOL_ERROR     | false     | Message violated protocol structure or sequencing        |
| IDENTITY_MISMATCH  | false     | Hello ID does not match token claims                     |
| INTEGRITY_CONFLICT | false     | Event payload differs from stored canonical event        |
| INTERNAL_ERROR     | true      | Transient server error; retry with backoff               |

---

### `epoch_reset_command`

Direction: Server -> Forwarder

| Field            | Type    | Notes                          |
|------------------|---------|--------------------------------|
| kind             | `"epoch_reset_command"` |                  |
| session_id       | string  |                                |
| forwarder_id     | string  |                                |
| reader_ip        | string  |                                |
| new_stream_epoch | integer | New epoch value (>= 2)         |

**Semantics:**
- `stream_epoch` advances to `new_stream_epoch`; `seq` restarts at `1`.
- Unacked events from older epochs remain replayable until drained.
- Forwarder confirms by sending a new `forwarder_hello` with updated epoch.
- If the forwarder is not connected, `POST /api/v1/streams/{id}/reset-epoch`
  returns HTTP 409.

---

## Sub-type Definitions

### ResumeCursor

```json
{
  "forwarder_id": "fwd-001",
  "reader_ip": "192.168.1.10",
  "stream_epoch": 1,
  "last_seq": 4200
}
```

### ReadEvent

```json
{
  "forwarder_id": "fwd-001",
  "reader_ip": "192.168.1.10",
  "stream_epoch": 1,
  "seq": 4201,
  "reader_timestamp": "2026-02-17T10:00:00.000Z",
  "raw_read_line": "09001234567890123 12:00:00.000 1",
  "read_type": "RAW"
}
```

`raw_read_line` is UTF-8 text. ASCII IPICO payload is expected. Invalid UTF-8
MUST be rejected — no silent byte replacement.

`reader_timestamp` is accepted as-is from the device. No server adjustment.

`read_type` is one of `"RAW"` or `"FSLS"` in v1.

### AckEntry

```json
{
  "forwarder_id": "fwd-001",
  "reader_ip": "192.168.1.10",
  "stream_epoch": 1,
  "last_seq": 4202
}
```

Acks use the high-water mark pattern: `last_seq` means all events up to and
including that sequence number on `(forwarder_id, reader_ip, stream_epoch)` are
acknowledged.

### StreamRef

```json
{
  "forwarder_id": "fwd-001",
  "reader_ip": "192.168.1.10"
}
```

---

## HTTP API

Base path: `/api/v1`

### `GET /api/v1/streams`

Returns the list of all known streams.

**Response 200:**
```json
[
  {
    "stream_id": "stream-uuid-here",
    "forwarder_id": "fwd-001",
    "reader_ip": "192.168.1.10",
    "display_alias": "Start",
    "stream_epoch": 1,
    "online": true
  }
]
```

| Status | Condition              |
|--------|------------------------|
| 200    | Success (may be empty) |
| 401    | Missing/invalid auth   |
| 500    | Internal error         |

---

### `PATCH /api/v1/streams/{stream_id}`

Update the display alias of a stream.

**Request body:**
```json
{ "display_alias": "Finish" }
```

**Response 200:** Updated stream object (same shape as `GET /api/v1/streams` entry).

| Status | Condition                      |
|--------|--------------------------------|
| 200    | Updated; returns updated stream |
| 400    | Malformed request body          |
| 401    | Missing/invalid auth            |
| 404    | Stream not found                |
| 500    | Internal error                  |

---

### `GET /api/v1/streams/{stream_id}/metrics`

Returns lifetime metrics for a stream.

**Response 200:**
```json
{
  "raw_count": 5000,
  "dedup_count": 4800,
  "retransmit_count": 200,
  "lag_ms": 1500,
  "backlog": 42
}
```

Invariant: `raw_count == dedup_count + retransmit_count`.

`lag_ms` is `null` when the stream has no canonical events.
`backlog` is `0` when there are no active receivers.

| Status | Condition              |
|--------|------------------------|
| 200    | Success                |
| 401    | Missing/invalid auth   |
| 404    | Stream not found       |
| 500    | Internal error         |

---

### `POST /api/v1/streams/{stream_id}/reset-epoch`

Instructs the server to send an `epoch_reset_command` to the connected forwarder.

**Response 200:**
```json
{ "new_stream_epoch": 4 }
```

| Status | Condition                                   |
|--------|---------------------------------------------|
| 200    | Reset command dispatched; returns new epoch  |
| 401    | Missing/invalid auth                         |
| 404    | Stream not found                             |
| 409    | Forwarder not currently connected            |
| 500    | Internal error                               |

---

### `GET /api/v1/streams/{stream_id}/export/raw`

Export canonical (deduplicated) events as raw lines.

**Format:** bare lines, one event per line, `\n`-terminated, no header.
Each line is the `raw_read_line` field of the canonical event.
Retransmits are excluded.

| Status | Condition            |
|--------|----------------------|
| 200    | Streaming body       |
| 401    | Missing/invalid auth |
| 404    | Stream not found     |
| 500    | Internal error       |

---

### `GET /api/v1/streams/{stream_id}/export/csv`

Export canonical (deduplicated) events as CSV.

**Format:**
- Header row: `stream_epoch,seq,reader_timestamp,raw_read_line,read_type`
- One data row per canonical event.
- RFC 4180 quoting: fields containing commas, double-quotes, or newlines are
  enclosed in double-quotes; literal double-quotes are escaped as `""`.
- Line terminator: `\n` (LF only).
- Retransmits are excluded.

Example:
```
stream_epoch,seq,reader_timestamp,raw_read_line,read_type
1,4201,2026-02-17T10:00:00.000Z,"09001234567890123 12:00:00.000 1",RAW
1,4202,2026-02-17T10:00:01.500Z,"09001234567890124 12:00:01.500 1",RAW
```

| Status | Condition            |
|--------|----------------------|
| 200    | Streaming body       |
| 401    | Missing/invalid auth |
| 404    | Stream not found     |
| 500    | Internal error       |

---

## HTTP Error Envelope

All non-2xx responses use the following JSON envelope:

```json
{
  "code": "NOT_FOUND",
  "message": "Stream stream-uuid-here not found.",
  "details": {}
}
```

| Field   | Type   | Required | Notes                         |
|---------|--------|----------|-------------------------------|
| code    | string | yes      | Machine-readable error code   |
| message | string | yes      | Human-readable description    |
| details | object | no       | Optional structured context   |

---

## Delivery Guarantees

- **At-least-once** end-to-end delivery.
- Retransmissions of the same event identity (same payload) increment
  `retransmit_count` but do not create a duplicate canonical row.
- If the same event identity arrives with **different payload bytes**, the
  server rejects it as `INTEGRITY_CONFLICT` and preserves the original
  canonical event. The metric row is NOT updated.
- Ordering and replay use event identity `(stream_epoch, seq)` sequence data.

---

## Guaranteed Backfill

- Target: 24 hours of offline data.
- If disk pressure prevents the 24-hour target, the forwarder continues
  ingesting and pruning oldest records, and MUST report a degraded-retention
  state explicitly (logged + health endpoint).

---

## Health Endpoints

Both server and forwarder expose:

| Endpoint  | Description                                                         |
|-----------|---------------------------------------------------------------------|
| /healthz  | Liveness — returns 200 if process is running                        |
| /readyz   | Readiness — returns 200 if all critical dependencies are ready      |

Forwarder `/readyz` does NOT require active uplink connectivity; it only
checks local dependencies (config, storage, worker loops).

---

## SQLite Durability Profile (Forwarder and Receiver)

```sql
PRAGMA journal_mode=WAL;
PRAGMA synchronous=FULL;
PRAGMA wal_autocheckpoint=1000;
PRAGMA foreign_keys=ON;
```

At startup, `PRAGMA integrity_check` MUST be run. If it fails, the process
MUST exit immediately with a clear error message.

---

## Deferred Scope (v2+)

- Reader control commands (time/stats/config/download).
- Forwarder local dashboard auth and config editing.
- Receiver multi-profile support.
- Multi-tenant server capability.
- Token revocation for already-connected sessions.
- Unsubscribe from streams.
