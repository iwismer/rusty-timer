---
type: is
id: is-01kj3bms18h8hvkhr9nr8btabf
title: "Task 12b: Build server-ui epoch mapping controls and component tests"
kind: task
status: open
priority: 2
version: 6
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies:
  - type: blocks
    target: is-01kj3b53n95tz8rd5g2ya5nehm
parent_id: is-01kj3b53n95tz8rd5g2ya5nehm
child_order_hints:
  - is-01kj3s9ws4g7fyqjfr84gsh4st
  - is-01kj3sa0m22yt38csfxapr9wn7
created_at: 2026-02-22T19:01:28.230Z
updated_at: 2026-02-22T23:00:28.197Z
---
Build server-ui stream epoch mapping controls and component tests with approved UX contract. Acceptance: (1) render epoch list on stream detail page; (2) each row has race selector with explicit Save action; (3) row-level dirty/pending/success/error feedback; (4) activate-next action remains available and correctly wired; (5) tests verify no API write occurs before row Save and verify error/retry behavior.

## Notes

Execution split under this bead:
- API helper additions for per-epoch mapping and activate-next.
- Per-row save workflow and UI state model.
- Component tests for per-row save semantics and activate-next control state.
