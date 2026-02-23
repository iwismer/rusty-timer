---
type: is
id: is-01kj5pha4a0ssbvdtgp4r3crt8
title: Forwarder UI never shows epoch name set on server — backend ReaderStatus struct and ReaderUpdated SSE event missing current_epoch_name
kind: bug
status: open
priority: 2
version: 1
labels: []
dependencies: []
created_at: 2026-02-23T16:50:17.865Z
updated_at: 2026-02-23T16:50:17.865Z
---

## Problem

The forwarder UI has a display cell for the current epoch name ("Active: {name}") but it is **never populated**. When an operator sets an epoch name on the server UI, it does not appear in the forwarder UI. The `current_epoch_name` field was added to the TypeScript `ReaderStatus` interface (rt-9tuf, closed), but the forwarder backend never populates or propagates the value.

## Root Cause

The forwarder backend's `ReaderStatus` and `ReaderStatusJson` structs do not have a `current_epoch_name` field. The `ForwarderUiEvent::ReaderUpdated` SSE event also does not carry the value, so the UI always receives `undefined`.

Additionally, when an operator sets an epoch name via the forwarder UI's own input, `set_current_epoch_name_handler` sends the name to the upstream server but does not update forwarder local state — the UI is not notified even of its own write.

Relevant code:
- `services/forwarder/src/status_http.rs:77-84` — `ReaderStatus` struct (missing field)
- `services/forwarder/src/status_http.rs:1499-1506` — `ReaderStatusJson` struct (missing field)
- `services/forwarder/src/ui_events.rs:11-18` — `ReaderUpdated` event variant (missing field)
- `services/forwarder/src/status_http.rs:1708-1854` — `set_current_epoch_name_handler` (no local state update after write)
- `apps/forwarder-ui/src/lib/api.ts:10` — `current_epoch_name?: string | null` already present (frontend-only fix from rt-9tuf)
- `apps/forwarder-ui/src/routes/+page.svelte:407-459` — display logic already in place

## Scope

**In scope:**
- Add `current_epoch_name: Option<String>` to forwarder backend structs and SSE event.
- After `set_current_epoch_name_handler` succeeds, store the value in reader local state and broadcast a `ReaderUpdated` SSE event.
- Populate `current_epoch_name` from local state in the status endpoint and resync path.

**Out of scope:** Real-time push from server→forwarder when name is changed from the server UI (requires a server-to-forwarder push channel; separate feature).

## Acceptance Criteria

- [ ] After operator sets epoch name via the forwarder UI input, the "Active: {name}" cell updates without a page refresh.
- [ ] The epoch name persists in the UI across SSE reconnects (populated in status endpoint and `ResyncEvent`).
- [ ] Setting the name to empty/null clears the "Active:" display.
- [ ] No regressions in existing forwarder UI SSE event handling.

## Technical Notes

1. Add `current_epoch_name: Option<String>` to `ReaderStatus` in `status_http.rs`.
2. Add `current_epoch_name: Option<String>` to `ReaderStatusJson` in `status_http.rs`.
3. Add `current_epoch_name: Option<String>` to `ForwarderUiEvent::ReaderUpdated` in `ui_events.rs`.
4. In `set_current_epoch_name_handler`: after successful PUT to upstream, store the name in per-reader state, then broadcast a `ReaderUpdated` event with the updated value.
5. In the status endpoint and resync path: populate `current_epoch_name` from local state.
6. Update forwarder-ui SSE handler (`apps/forwarder-ui/src/lib/sse.ts`) to propagate the field if not already handled by the existing `onReaderUpdated` callback.

## Risks

- The forwarder does not currently track epoch name in local state; a storage location must be added alongside other per-reader state.
- If the server changes the epoch name externally (not via forwarder UI), the forwarder UI will remain stale. This is explicitly deferred.

## Validation

- Manual: set epoch name via forwarder UI input, confirm "Active: {name}" appears immediately.
- Manual: close and reopen the forwarder UI tab, confirm name is still shown (populated via status endpoint).
- Automated: unit test for `set_current_epoch_name_handler` asserting that a `ReaderUpdated` event is broadcast with the correct `current_epoch_name`.
