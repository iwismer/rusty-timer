# Announcer Screen Design

Date: 2026-02-26
Status: Approved

## Goal

Add a public `/announcer` screen that shows newly detected finishers in near-real-time for announcers, while keeping management in the existing server dashboard UX.

## Scope

- Public announcer display page at `/announcer` (no login gate in this feature).
- New authenticated-in-practice (existing LAN-trust dashboard model) announcer config tab in Server UI.
- First-read-per-chip behavior across selected streams.
- Live updates with highlight animation and rolling list.

Out of scope:

- Introducing new auth model for dashboard APIs.
- Official timing/place calculations.
- Persisting announcer runtime state across server restarts.

## Final Product Decisions

- Per-stream race resolution for name/bib mapping.
- Stream selection changes reset announcer dedup/list/count immediately.
- Disabled announcer shows a public "Announcer screen is disabled" page.
- Selected stream epoch changes auto-reset announcer dedup/list/count.
- Enabling requires at least one selected stream.
- When list reaches max size, keep most recent entries and drop oldest.
- List ordering is newest at top.
- Unknown mappings are shown as `Unknown` with chip ID.
- Deterministic first-read tie-break: `received_at`, then `stream_id`, then `seq`.
- Announcer runtime state resets on server restart.
- Live updates use SSE.
- 24-hour expiry is fixed from explicit enable action; config edits do not extend it.
- Enable/reset does not backfill history; only new reads are considered.
- Keep current LAN-trust dashboard model for this feature.

## Architecture

### Backend

Persisted config (DB-backed):

- `enabled` (bool)
- `enabled_until` (timestamp)
- `selected_stream_ids` (array of stream UUIDs)
- `max_list_size` (int, default 25)

In-memory runtime (server process state):

- `seen_chips` set for dedup
- bounded newest-first announcer rows
- `finisher_count`
- selected stream epoch snapshot (for auto-reset detection)

New endpoints:

- `GET /announcer` public page
- `GET /api/v1/announcer/state` initial state snapshot
- `GET /api/v1/announcer/events` SSE stream
- `GET /api/v1/announcer/config` dashboard config read
- `PUT /api/v1/announcer/config` dashboard config write
- `POST /api/v1/announcer/reset` dashboard manual reset

### Frontend

Server UI:

- Add Announcer tab with:
  - enable checkbox
  - selected stream multi-select
  - max list size input (default 25)
  - reset button
  - expiry visibility (enabled until)

Public announcer page:

- disclaimer banner (unofficial results/times/placements)
- newest-first finisher list
- green flash/fade for newly arrived rows
- total finisher count footer
- disabled-state message page

## Data Flow

1. Operator saves announcer config in dashboard.
2. Backend validates and persists config.
3. On enable or reset-triggering changes, backend clears announcer runtime state.
4. Incoming canonical read events are filtered to selected streams and announcer enabled/expiry state.
5. First unseen chip (deterministic ordering) is resolved to participant data via stream -> forwarder race -> chips/participants.
6. Backend prepends announcer row, trims to max size, increments finisher count, emits SSE update.
7. `/announcer` clients hydrate from snapshot endpoint and remain live via SSE.

Reset triggers:

- manual reset endpoint
- selected stream set changed
- selected stream epoch changed

## Validation and Error Handling

- Reject enabling when selected stream list is empty.
- Reject unknown stream IDs.
- Clamp `max_list_size` to safe range (proposed `1..=500`), default `25`.
- Unknown participant mapping still emits visible row as `Unknown` + chip ID.
- SSE disconnects only affect client delivery; runtime accumulation continues.
- Expired config behaves as disabled for read ingestion and `/announcer` display.
- `PUT /api/v1/announcer/config` is atomic: persist config and apply/reset runtime together.
- Shared reset implementation for all reset causes to avoid drift.

## Testing Strategy

Backend unit tests:

- first-read dedup correctness
- deterministic tie-break correctness
- newest-first trim behavior
- unknown mapping formatting
- expiry gating

Backend integration tests:

- enable validation (non-empty streams)
- config update reset semantics
- epoch-change reset semantics
- manual reset behavior
- disabled page behavior
- SSE snapshot + incremental event behavior

Frontend tests:

- announcer config tab render/save/reset/validation errors
- public `/announcer` render paths (enabled/disabled)
- newest-first list ordering
- flash/fade class behavior on new row
- finisher count updates

Manual smoke:

- enable announcer, generate emulator reads, view on phone/device
- verify 24-hour auto-disable with controlled time in test harness

## Operational Notes

- This feature follows existing LAN-trust model used by dashboard APIs.
- Because runtime is in-memory, server restart clears announcer list/count/seen-chip state by design.
- Announcer data is explicitly non-official and non-authoritative.
