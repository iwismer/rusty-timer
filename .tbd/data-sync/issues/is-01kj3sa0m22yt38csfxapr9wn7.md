---
type: is
id: is-01kj3sa0m22yt38csfxapr9wn7
title: "Task 12d: server-ui activate-next integration and tests"
kind: task
status: closed
priority: 2
version: 5
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
assignee: subagent-server-ui-phase3
labels: []
dependencies: []
parent_id: is-01kj3bms18h8hvkhr9nr8btabf
created_at: 2026-02-22T23:00:15.616Z
updated_at: 2026-02-23T00:36:05.964Z
closed_at: 2026-02-23T00:36:05.962Z
close_reason: "Implemented and reviewed: activate-next row-state race guard + restored root-page smoke coverage. Independent review APPROVE; integrated as commit 64a02f4."
---
Wire activate-next control in server-ui stream detail to new epoch mapping workflow. Scope: apps/server-ui/src/lib/api.ts, src/routes/streams/[streamId]/+page.svelte, src/lib/api.test.ts, src/routes/streams/[streamId]/+page.svelte.test.ts. Add pending/error states and tests for disabled/busy behavior while requests are in-flight.

## Notes

Independent review CHANGES_REQUESTED: (1) high race-safety bug in activate-next async completion overwriting newer row state, (2) medium coverage regression from replacing root-page smoke test.
