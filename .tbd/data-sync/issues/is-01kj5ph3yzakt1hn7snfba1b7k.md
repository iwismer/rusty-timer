---
type: is
id: is-01kj5ph3yzakt1hn7snfba1b7k
title: Epoch race mapping table event count does not auto-update as new reads arrive
kind: bug
status: open
priority: 3
version: 1
labels: []
dependencies: []
created_at: 2026-02-23T16:50:11.550Z
updated_at: 2026-02-23T16:50:11.550Z
---

## Problem

The "Events" column in the epoch race mapping table on the stream details page shows a stale count. It is loaded once on page load (or on epoch advance) and does not refresh as new reads come in from connected readers. An operator watching the page during a live race will see a frozen event count until the epoch changes.

## Root Cause

`loadEpochRaceRows()` is only triggered by:
1. Initial page load.
2. `stream_updated` SSE event **with a `stream_epoch` change**.

New reads arriving at the current epoch do not emit a `stream_updated` event, so the count never updates. The count is sourced from `COUNT(e.stream_id)` in the `list_epochs` SQL query (`services/server/src/http/streams.rs:159-185`).

Relevant code:
- `apps/server-ui/src/routes/streams/[streamId]/+page.svelte:75-84` — SSE-triggered reload (only fires on epoch change)
- `apps/server-ui/src/routes/streams/[streamId]/+page.svelte:212-274` — `loadEpochRaceRows`
- `services/server/src/http/streams.rs:159-185` — SQL query that counts events

## Scope

**In scope:** The "Events" column in the epoch race mapping table auto-refreshes during a live race.

**Out of scope:** Real-time per-read streaming (individual reads do not need to appear instantly). An update every N seconds is acceptable.

## Proposed Approach

Periodic polling on the stream details page is the lowest-risk path. The page already uses a `setInterval` pattern for "time since last read" (lines 146-152). Add a separate interval (e.g., every 5 seconds) that calls `loadEpochRaceRows` while the page is mounted.

Alternative (more complex): emit a new SSE event from the server whenever an event batch is persisted, and listen for it in the UI to trigger a selective reload. This would require changes to `services/server/src/repo/events.rs` and `services/server/src/dashboard_events.rs`.

## Acceptance Criteria

- [ ] The Events count in the epoch race mapping table updates without a page reload during an active race.
- [ ] Updates occur at most every ~5 seconds (polling acceptable; real-time not required).
- [ ] Polling does not cause visible flicker or disrupt unsaved race/name edits in the same table.
- [ ] No polling occurs when the page is not in focus — use `visibilitychange` or `document.hidden` to pause/resume.

## Technical Notes

- `apps/server-ui/src/routes/streams/[streamId]/+page.svelte:146-152` — reference pattern for existing interval.
- The reload must not overwrite `epochRaceRows` rows that have `pending: true` or `dirty: true` (unsaved edits). Merge carefully.

## Validation

- Manual: open stream details page during a live read session; confirm event count increments within ~5 seconds.
- Automated: mock the API response and confirm the interval triggers `loadEpochRaceRows` at the expected cadence.
