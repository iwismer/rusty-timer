---
type: is
id: is-01kj5ph0m22458s1ay6qhcgxyx
title: Epoch advance button reverts to idle state while table is still reloading — no visual indicator during SSE-driven data refresh
kind: bug
status: closed
priority: 3
version: 3
labels: []
dependencies: []
created_at: 2026-02-23T16:50:08.129Z
updated_at: 2026-02-23T19:31:46.408Z
closed_at: 2026-02-23T19:31:46.407Z
close_reason: "PR #101 created. Epoch advance button now stays in loading state during SSE table reload."
---
## Problem

When an operator clicks "Advance to Next Epoch" on the stream details page, the button shows "Advancing..." only while the POST request is in-flight. As soon as the API returns 204, `epochAdvancePending` is reset to `false` and the button reverts to "Advance to Next Epoch" — but the epoch race mapping table has **not yet updated**. The table refreshes only after the server emits a `stream_updated` SSE event and `loadEpochRaceRows()` completes (a round-trip that takes a noticeable 1–2 seconds). During this gap there is no feedback, making it look like nothing happened.

## Reproduction

1. Open stream details page with an epoch that has a saved race mapping.
2. Click "Advance to Next Epoch".
3. Observe: button briefly shows "Advancing…", then immediately returns to "Advance to Next Epoch".
4. The table epoch number does not update for ~1–2 seconds.

## Root Cause

`handleAdvanceToNextEpoch()` sets `epochAdvancePending = false` in the `finally` block, which runs when the API call returns. The SSE-triggered `loadEpochRaceRows()` runs independently and can take another 1–2 seconds. There is no loading state for this second phase.

Relevant code:
- `apps/server-ui/src/routes/streams/[streamId]/+page.svelte:376-389` — `handleAdvanceToNextEpoch`
- `apps/server-ui/src/routes/streams/[streamId]/+page.svelte:75-84` — SSE listener that triggers `loadEpochRaceRows`
- `apps/server-ui/src/routes/streams/[streamId]/+page.svelte:50-52` — `epochAdvancePending` / `epochAdvanceStatus` state

## Scope

**In scope:** Maintain a visual "reloading" state on the button (or table area) from the time the API call returns until `loadEpochRaceRows` completes.

**Out of scope:** Changes to SSE infrastructure, backend, or other table interactions.

## Acceptance Criteria

- [ ] After clicking "Advance to Next Epoch", some loading indicator persists until the epoch race mapping table reflects the new epoch (i.e., until `loadEpochRaceRows` resolves post-SSE).
- [ ] The button is disabled / shows loading during the entire operation (API call + reload), not just during the POST.
- [ ] Clicking the button a second time while reload is pending is prevented.
- [ ] Error state ("Advance failed") still works correctly.
- [ ] Existing test in `stream-detail-page.svelte.test.ts:429-444` continues to pass; update if needed.

## Technical Notes

- Introduce a second state variable (e.g., `epochAdvanceReloading`) set `true` after the API returns and cleared after `loadEpochRaceRows` resolves.
- Or extend `epochAdvancePending` to remain `true` until the post-SSE reload completes by passing a callback or awaiting a promise from the reload path.
- The SSE listener at line 75-84 currently calls `loadEpochRaceRows` without coordination; this needs to coordinate with the pending advance state.

## Validation

- Manual: advance epoch, confirm button stays in loading state until table updates.
- Automated: update/add Svelte test to assert button stays disabled until `loadEpochRaceRows` resolves.
