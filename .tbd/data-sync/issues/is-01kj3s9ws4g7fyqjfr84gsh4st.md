---
type: is
id: is-01kj3s9ws4g7fyqjfr84gsh4st
title: "Task 12c: server-ui epoch row save workflow"
kind: task
status: open
priority: 2
version: 2
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies:
  - type: blocks
    target: is-01kj3sa0m22yt38csfxapr9wn7
parent_id: is-01kj3bms18h8hvkhr9nr8btabf
created_at: 2026-02-22T23:00:11.683Z
updated_at: 2026-02-22T23:00:33.483Z
---
Implement stream-epoch mapping row workflow in server-ui stream detail page with explicit Save action per row. Scope: apps/server-ui/src/routes/streams/[streamId]/+page.svelte, src/lib/api.ts, src/routes/streams/[streamId]/+page.svelte.test.ts. Add per-row dirty/pending/success/error indicators and retry path. Ensure no mapping API call is made before Save.
