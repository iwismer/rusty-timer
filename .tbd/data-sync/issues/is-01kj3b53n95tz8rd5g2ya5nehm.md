---
type: is
id: is-01kj3b53n95tz8rd5g2ya5nehm
title: "Task 12: Add server UI mapping and activation controls plus component tests"
kind: task
status: closed
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
  - is-01kj3bmrtzbhstn53wrjbyb5pd
  - is-01kj3bms18h8hvkhr9nr8btabf
created_at: 2026-02-22T18:52:54.823Z
updated_at: 2026-02-23T01:02:33.113Z
closed_at: 2026-02-23T01:02:33.112Z
close_reason: "Task 12 completed: infra/task splits rt-sohq + rt-pg8n closed, per-row save mapping + activate-next behavior implemented, independently reviewed, and UI suites pass in PR #89."
---
From implementation plan Task 12 (docs/plans/2026-02-22-race-epoch-receiver-v1.1-implementation-plan.md).

## Notes

UI plan approved by product (2026-02-22): server-ui owns stream epoch -> race mapping and activation controls.

Decisions:
- Epoch mapping is edited per row with an explicit Save action per row.
- Row-level UX must expose dirty/pending/success/error state.
- Keep activate-next on stream detail page aligned with selected race mapping workflows.

Non-goals for this task:
- No auto-save-on-change for server-ui epoch mapping rows.
- No receiver replay controls in server-ui.
- No bulk save-all-rows requirement in this iteration.
