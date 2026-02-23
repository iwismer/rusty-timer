---
type: is
id: is-01kj5n5ysknmf5fy65csp047my
title: Show human-readable stream name in cursor reset table on receiver admin page
kind: task
status: closed
priority: 3
version: 4
labels: []
dependencies: []
created_at: 2026-02-23T16:26:37.233Z
updated_at: 2026-02-23T19:31:47.438Z
closed_at: 2026-02-23T19:31:47.437Z
close_reason: "PR #99 created. Stream display_alias shown in admin cursor reset table."
---
## Problem / Context

The cursor reset stream table on the receiver admin page (/admin) has a Stream column that uses streamLabel() to display stream identity. However, display_alias (the human-readable name) is optional on StreamEntry and may not be included in the admin endpoint response payload — causing the helper to silently fall back to forwarder_id / reader_ip for all rows even when a human-readable name exists.

Operators resetting cursors need to identify streams at a glance, not decode forwarder/reader IP combinations.

## Scope

- Verify the cursor reset endpoint response includes display_alias when available
- If missing from the response, add it to the backend serialization
- Ensure the Stream column in the cursor reset table prominently shows the alias when present, falling back to the existing label otherwise

## Non-Goals

- Do not change the Forwarder ID or Reader IP columns
- Do not change the cursor reset action itself or any API behavior

## Acceptance Criteria

- [ ] When a stream has a display_alias, the cursor reset table's Stream column shows it
- [ ] When no alias exists, the existing fallback label (forwarder_id / reader_ip) is unchanged
- [ ] Admin page renders correctly for streams both with and without a display_alias

## Technical Notes

- apps/receiver-ui/src/routes/admin/+page.svelte: cursor reset table at lines 97-128; streamLabel() helper at lines 16-20
- services/receiver/src/control_api.rs: StreamEntry struct with optional display_alias field (~line 288); verify it is populated in the admin stream list query
- apps/receiver-ui/src/lib/api.ts: StreamEntry interface with display_alias?: string (~line 20)

## Risks & Edge Cases

- The admin endpoint might use a different stream fetch path than the main page, one that does not join on the alias source — low blast radius since this is purely additive display data

## Validation Plan

- [ ] Set a display_alias on a stream; visit the admin page; confirm the alias appears in the cursor reset table Stream column
- [ ] Remove the alias; confirm the fallback label shows correctly
- [ ] Confirm no regression on cursor reset functionality itself
