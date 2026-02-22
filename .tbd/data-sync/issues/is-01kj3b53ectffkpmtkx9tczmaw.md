---
type: is
id: is-01kj3b53ectffkpmtkx9tczmaw
title: "Task 11: Add receiver UI selection controls plus component-level behavior tests"
kind: task
status: in_progress
priority: 2
version: 9
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies:
  - type: blocks
    target: is-01kj3b542wtv96m2e8t6jmsrec
  - type: blocks
    target: is-01kj3bmstkkwd2f9qkp3gw9yn7
parent_id: is-01kj3b514za6r2c96xxn5w3wcn
child_order_hints:
  - is-01kj3bmqvgssejwpsw9xczny6s
  - is-01kj3bmr21hx8q7yxg8j53q24q
created_at: 2026-02-22T18:52:54.602Z
updated_at: 2026-02-22T22:59:44.593Z
---
From implementation plan Task 11 (docs/plans/2026-02-22-race-epoch-receiver-v1.1-implementation-plan.md). Include receiver UI behavior/tests that default selection mode to manual and require explicit opt-in to race mode.

## Notes

UI plan approved by product (2026-02-22): receiver-ui owns replay selection controls.

Decisions:
- Controls are auto-apply on committed control changes (mode, race, epoch scope, replay policy).
- Targeted replay uses a table editor.
- Stream choice in targeted table is dropdown-only from discovered streams (no free-text forwarder_id/reader_ip entry).
- Maintain clear saving/error feedback to avoid operator confusion from auto-apply behavior.

Non-goals for this task:
- No raw JSON editor for replay targets.
- No manual/ad-hoc stream identifier entry in targeted rows.
- No combined server-ui ownership for replay controls.
