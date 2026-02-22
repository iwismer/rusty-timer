---
type: is
id: is-01kj3s9s5pkwxz1ehp8gp1njj6
title: "Task 11d: receiver-ui targeted replay table editor (dropdown-only)"
kind: task
status: open
priority: 2
version: 2
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
assignee: subagent-receiver-ui-phase3
labels: []
dependencies: []
parent_id: is-01kj3bmr21hx8q7yxg8j53q24q
created_at: 2026-02-22T23:00:07.988Z
updated_at: 2026-02-22T23:11:41.264Z
---
Implement targeted replay table editor in receiver-ui using dropdown-only stream selection from discovered streams. Scope: apps/receiver-ui/src/routes/+page.svelte, src/lib/api.ts, src/routes/+page.svelte.test.ts. Include add/remove row, epoch/from-seq editing, row validation, and request serialization. Invalid rows must surface inline errors and must not be submitted.

## Notes

QUEUED after rt-svgl\nAssignment packet: .context/subagent-assignments/rt-c3vd-receiver-ui-targeted-table.md
