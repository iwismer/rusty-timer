---
type: is
id: is-01kj3s9ws4g7fyqjfr84gsh4st
title: "Task 12c: server-ui epoch row save workflow"
kind: task
status: closed
priority: 2
version: 6
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
assignee: subagent-server-ui-phase2
labels: []
dependencies:
  - type: blocks
    target: is-01kj3sa0m22yt38csfxapr9wn7
parent_id: is-01kj3bms18h8hvkhr9nr8btabf
created_at: 2026-02-22T23:00:11.683Z
updated_at: 2026-02-23T00:13:36.504Z
closed_at: 2026-02-23T00:13:36.503Z
close_reason: Implemented via commits 975c047, 1516667, f1fe02b (from 5ac7837/06a4729/d62ff50), independently reviewed (APPROVE), and integration validations passed in apps/server-ui.
---
Implement stream-epoch mapping row workflow in server-ui stream detail page with explicit Save action per row. Scope: apps/server-ui/src/routes/streams/[streamId]/+page.svelte, src/lib/api.ts, src/routes/streams/[streamId]/+page.svelte.test.ts. Add per-row dirty/pending/success/error indicators and retry path. Ensure no mapping API call is made before Save.

## Notes

Remediation loop 2 required after independent review (session 34284) found false 'Saved' state on incomplete hydration edit/revert path.
