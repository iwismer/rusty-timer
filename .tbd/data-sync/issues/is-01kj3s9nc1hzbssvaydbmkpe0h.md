---
type: is
id: is-01kj3s9nc1hzbssvaydbmkpe0h
title: "Task 11c: receiver-ui auto-apply selection controls wiring"
kind: task
status: open
priority: 2
version: 3
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
assignee: subagent-receiver-ui-phase2
labels: []
dependencies:
  - type: blocks
    target: is-01kj3s9s5pkwxz1ehp8gp1njj6
parent_id: is-01kj3bmr21hx8q7yxg8j53q24q
created_at: 2026-02-22T23:00:04.095Z
updated_at: 2026-02-22T23:11:36.543Z
---
Implement auto-apply flow for mode/race/scope/replay-policy controls in receiver-ui. Scope: apps/receiver-ui/src/routes/+page.svelte, src/lib/api.ts, src/lib/api.test.ts, src/lib/sse.ts. Must avoid request storms (apply on committed control changes, not per keystroke). Add/adjust component tests for payload correctness and error rollback behavior.

## Notes

QUEUED after rt-iex3\nAssignment packet: .context/subagent-assignments/rt-svgl-receiver-ui-auto-apply.md
