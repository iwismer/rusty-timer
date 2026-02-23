---
type: is
id: is-01kj5p51a300tz8cvdd88kdpeh
title: apiFetch drops Content-Type when caller passes custom headers, breaking cursor reset
kind: bug
status: closed
priority: 2
version: 4
labels: []
dependencies: []
created_at: 2026-02-23T16:43:35.618Z
updated_at: 2026-02-23T18:56:20.621Z
closed_at: 2026-02-23T18:56:20.619Z
close_reason: "Fixed in PR #97"
---
## Bug

POST /api/v1/admin/cursors/reset returns 'Expected request with Content-Type: application/json' even though apiFetch is supposed to set it automatically.

## Root Cause

In apps/shared-ui/src/lib/api-helpers.ts, the fetch call is structured as:

  const resp = await fetch(path, {
    headers: { 'Content-Type': 'application/json', ...(init?.headers ?? {}) },
    ...init,
  });

The spread ...init comes AFTER the explicit headers key. In JavaScript, when an object literal contains duplicate keys, the last one wins. So when init contains a headers property (e.g. resetStreamCursor passes { 'x-rt-receiver-admin-intent': '...' }), the ...init spread overwrites the entire merged headers object — dropping Content-Type entirely.

Calls that do not pass custom headers (e.g. putSelection) are unaffected because init.headers is undefined, so ...init does not produce a headers key.

## Fix

Swap the order so headers is set after ...init:

  const resp = await fetch(path, {
    ...init,
    headers: { 'Content-Type': 'application/json', ...(init?.headers ?? {}) },
  });

This ensures the merged headers object always wins, regardless of what init contains.

## Affected Calls

Any apiFetch call that passes a custom headers object. Currently:
- api.ts resetStreamCursor() — passes { 'x-rt-receiver-admin-intent': 'reset-stream-cursor' }

## Non-Goals

- No changes to callers; fix is entirely in api-helpers.ts

## Acceptance Criteria

- [ ] Clicking Reset Cursor on the admin page no longer returns 'Expected request with Content-Type: application/json'
- [ ] The cursor is actually reset (204 response)
- [ ] apiFetch calls without custom headers continue to work correctly (putSelection, putSubscriptions, etc.)
- [ ] Existing api-helpers tests pass; add a test case covering calls with custom headers to prevent regression

## Technical Notes

- apps/shared-ui/src/lib/api-helpers.ts — one-line fix (swap order of ...init and headers)
- apps/shared-ui/src/lib/api-helpers.test.ts — add regression test

## Risks

- None. The fix is a line reorder with no behavior change for calls without custom headers.
