---
type: is
id: is-01kj3sa0m22yt38csfxapr9wn7
title: "Task 12d: server-ui activate-next integration and tests"
kind: task
status: open
priority: 2
version: 1
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies: []
parent_id: is-01kj3bms18h8hvkhr9nr8btabf
created_at: 2026-02-22T23:00:15.616Z
updated_at: 2026-02-22T23:00:15.616Z
---
Wire activate-next control in server-ui stream detail to new epoch mapping workflow. Scope: apps/server-ui/src/lib/api.ts, src/routes/streams/[streamId]/+page.svelte, src/lib/api.test.ts, src/routes/streams/[streamId]/+page.svelte.test.ts. Add pending/error states and tests for disabled/busy behavior while requests are in-flight.
