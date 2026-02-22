# Race/Epoch Receiver Selection v1.1 Design

Date: 2026-02-22
Status: Approved
Owners: server + receiver + protocol

## Summary

Introduce a protocol and server-side subscription model (`/ws/v1.1/receivers`) that lets a receiver select either:

- manual stream subscriptions (existing behavior), or
- race-based subscriptions with epoch scope (`all` or `current`).

The server becomes authoritative for resolving selected race mappings into `(stream, epoch)` targets, replaying missing events from the selected epoch starts, and filtering live delivery.

This design keeps both mapping models:

- `forwarder -> race` (existing; retained for participant/chip enrichment), and
- `stream-epoch -> race` (new; used for receiver auto-subscription behavior).

Mismatch between the two is warn-only.

## Confirmed Decisions

- Protocol may introduce breaking changes in `v1.1` (not yet deployed to production).
- A `(stream, epoch)` maps to at most one race.
- Receiver can select only one race at a time.
- Receiver supports two race epoch scopes: `all` and `current`.
- Backfill rule: for selected epoch(s), replay missing reads from the start of selected epoch(s).
- Removing and re-adding an epoch should resume from prior per-epoch cursor, not replay from seq=1.
- If no race is selected, receiver falls back to manual per-stream behavior.
- Keep `forwarder -> race` assignment for ppl/bibchip behavior.
- Mapping mismatch between forwarder-race and epoch-race is warn-only.
- Exactly one next epoch may be pre-created per stream and immediately activated.
- Activation fails immediately if forwarder is offline (no queueing).

## Goals

- Support race-centric receiver workflows without requiring manual stream selection.
- Support both race epoch scopes (`all`, `current`) with deterministic behavior.
- Preserve at-least-once delivery and existing ack/cursor semantics.
- Preserve manual mode behavior as a fallback.

## Non-goals

- No automatic reconciliation between `forwarder_races` and epoch mappings.
- No multi-race simultaneous selection in a receiver session.
- No deferred/pending epoch activation for offline forwarders.

## Chosen Approach

Server-managed race selection and filtering.

Reasoning:

- Keeps receiver simple and deterministic.
- Centralizes mapping resolution and replay behavior in one place.
- Avoids polling/race conditions from receiver-side mapping expansion.

## Protocol Design (Receiver WS v1.1)

### Endpoint

- New endpoint: `GET /ws/v1.1/receivers`

`/ws/v1/receivers` remains unchanged for legacy/manual behavior.

### Handshake

- Client still sends `receiver_hello` first.
- Server still responds with heartbeat containing `session_id`.
- `receiver_hello` in v1.1 includes required `selection`.

### Selection model

Selection is a tagged union:

```json
{ "mode": "manual", "streams": [{ "forwarder_id": "f1", "reader_ip": "10.0.0.1:10000" }] }
```

```json
{ "mode": "race", "race_id": "<uuid>", "epoch_scope": "all" }
```

```json
{ "mode": "race", "race_id": "<uuid>", "epoch_scope": "current" }
```

### Mid-session updates

- Replace `receiver_subscribe` with `receiver_set_selection` (full replace semantics).

### Server acknowledgement

- Add server message `receiver_selection_applied` with:
  - normalized selection,
  - resolved target counts,
  - warning list (including forwarder/epoch mapping mismatches).

### Unchanged semantics

- `receiver_event_batch`, `receiver_ack`, heartbeat, and error handling remain the same.

## Data Model and API Design

### New mapping table

`stream_epoch_races`

- `stream_id UUID NOT NULL REFERENCES streams(stream_id) ON DELETE CASCADE`
- `stream_epoch BIGINT NOT NULL`
- `race_id UUID NOT NULL REFERENCES races(race_id) ON DELETE CASCADE`
- `created_at TIMESTAMPTZ NOT NULL DEFAULT now()`
- PK `(stream_id, stream_epoch)`
- Index on `race_id`

Invariant: a `(stream_id, stream_epoch)` can map to only one race.

### Receiver cursor schema update

Current server cursor key is effectively `(receiver_id, stream_id)`. Update to per-epoch persistence:

- PK `(receiver_id, stream_id, stream_epoch)`

This ensures remove/re-add of an epoch resumes from prior epoch progress.

### Keep forwarder race mapping

- Keep `forwarder_races` table and existing endpoints.
- Continue to use it for reads enrichment behavior.
- Do not auto-sync with `stream_epoch_races`.

### HTTP endpoints

Add/extend:

- `PUT /api/v1/streams/:stream_id/epochs/:epoch/race`
  - body `{ "race_id": "<uuid>" | null }`
  - set/unset mapping for one stream epoch.

- `GET /api/v1/races/:race_id/stream-epochs`
  - list mapped `(stream_id, forwarder_id, reader_ip, stream_epoch)` for race.

- `POST /api/v1/races/:race_id/streams/:stream_id/epochs/activate-next`
  - validates forwarder online,
  - computes `next = streams.stream_epoch + 1`,
  - creates/sets `stream_epoch_races(stream_id, next, race_id)`,
  - sends `EpochResetCommand` immediately,
  - advances `streams.stream_epoch` immediately.
  - if forwarder offline: `409 Conflict`.

- `GET /api/v1/streams/:stream_id/epochs`
  - include mapped-but-empty epochs (today endpoint only sees epochs with events).

### Single-next-epoch rule

Only `current + 1` can be activated/mapped by `activate-next`; skipping ahead is not allowed.

## Runtime Behavior

### Selection resolution

For each receiver session in v1.1, store current selection and resolved targets:

- `manual`: stream set from payload.
- `race/all`: all mapped `(stream, epoch)` rows for selected race.
- `race/current`: mapped streams for race where selected epoch equals each stream's current `streams.stream_epoch`.

### Backfill behavior

On `receiver_hello` and every `receiver_set_selection`:

- resolve targets,
- diff against prior targets,
- replay for newly active targets from epoch start, filtered by receiver cursor.

Cursor behavior:

- if cursor exists for `(receiver, stream, epoch)`, replay `seq > last_seq`.
- if cursor missing, replay from `seq >= 1`.

### Live filtering behavior

- `manual`: unchanged, current stream-level behavior.
- `race/all`: deliver only events whose `(stream, epoch)` is mapped to selected race.
- `race/current`: deliver only events for selected race streams whose epoch equals current stream epoch.

### Dynamic updates while connected

Re-resolve race-mode targets when any of these occur:

- `stream_epoch_races` changes for selected race,
- `streams.stream_epoch` changes (current epoch moves),
- race selection changes.

Removed targets stop live delivery immediately. Cursors remain persisted.

## Warn-only mismatch handling

When `forwarder_races` and `stream_epoch_races` imply different races for the same forwarder context:

- emit warning logs,
- emit dashboard warning event,
- include warning in `receiver_selection_applied` when relevant,
- do not reject, mutate, or auto-correct assignments.

## Error handling

- Invalid selection payload: protocol error (`PROTOCOL_ERROR`).
- Unknown race in selection: explicit error response; keep prior selection active.
- `activate-next` with forwarder offline: `409 Conflict`.
- Duplicate or conflicting mapping writes: DB constraint error surfaced as `409`/`400` based on cause.

## Testing plan

### Protocol/server

- v1.1 hello requires valid selection.
- `receiver_set_selection` fully replaces prior selection.
- `receiver_selection_applied` contains expected warnings and counts.

### Replay/cursor

- `race/all` replays all selected mapped epochs with per-epoch cursor filtering.
- remove/re-add epoch resumes from saved cursor.
- no cursor starts at epoch beginning.

### Current epoch behavior

- `race/current` follows `streams.stream_epoch` changes.
- `activate-next` moves current epoch immediately even before first read.

### API/data

- `(stream,epoch)` uniqueness enforced.
- `GET /streams/:id/epochs` includes mapped empty epochs.
- forwarder offline activation returns conflict.

### Regression

- manual mode behavior unchanged in v1.1.
- existing `/ws/v1/receivers` behavior unchanged.

## Migration and rollout

- Add DB migration for `stream_epoch_races` and cursor PK change.
- Add v1.1 protocol types and endpoint while retaining v1.
- Update receiver client to use v1.1 selection flow.
- Roll out UI/API support for mapping and activate-next.

