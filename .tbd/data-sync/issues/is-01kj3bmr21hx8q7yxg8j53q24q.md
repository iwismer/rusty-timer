---
type: is
id: is-01kj3bmr21hx8q7yxg8j53q24q
title: "Task 11b: Implement receiver-ui selection controls and component tests"
kind: task
status: open
priority: 2
version: 7
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies:
  - type: blocks
    target: is-01kj3b53ectffkpmtkx9tczmaw
parent_id: is-01kj3b53ectffkpmtkx9tczmaw
child_order_hints:
  - is-01kj3s9nc1hzbssvaydbmkpe0h
  - is-01kj3s9s5pkwxz1ehp8gp1njj6
created_at: 2026-02-22T19:01:27.231Z
updated_at: 2026-02-22T23:00:27.999Z
---
Implement receiver-ui selection controls and component tests with approved UX contract. Acceptance: (1) selection controls (manual/race, race id, epoch scope, replay policy) auto-apply on committed changes; (2) targeted replay uses table editor; (3) stream targets are chosen from dropdown only; (4) invalid targeted rows are not submitted; (5) component tests cover auto-apply request behavior and targeted-row validation/serialization.

## Notes

Execution split under this bead:
- API/client + state wiring for auto-apply and replay policy updates.
- Targeted replay table editor with dropdown stream selector and row validation.
- Component tests for state transitions, request payloads, and error handling.
