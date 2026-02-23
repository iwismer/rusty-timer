---
type: is
id: is-01kj3s9s5pkwxz1ehp8gp1njj6
title: "Task 11d: receiver-ui targeted replay table editor (dropdown-only)"
kind: task
status: closed
priority: 2
version: 4
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
assignee: subagent-receiver-ui-phase3
labels: []
dependencies: []
parent_id: is-01kj3bmr21hx8q7yxg8j53q24q
created_at: 2026-02-22T23:00:07.988Z
updated_at: 2026-02-23T00:15:59.103Z
closed_at: 2026-02-23T00:15:59.101Z
close_reason: Implemented via commit 9633d9a (from c5ab749), independently reviewed (APPROVE), and receiver-ui integration validations passed.
---
Implement targeted replay table editor in receiver-ui using dropdown-only stream selection from discovered streams. Scope: apps/receiver-ui/src/routes/+page.svelte, src/lib/api.ts, src/routes/+page.svelte.test.ts. Include add/remove row, epoch/from-seq editing, row validation, and request serialization. Invalid rows must surface inline errors and must not be submitted.

## Notes

DISPATCH ACTIVE: targeted replay table editor running in receiver-ui worktree after rt-svgl approval.
