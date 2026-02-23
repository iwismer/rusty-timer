---
type: is
id: is-01kj3bmm6zwfzsp2w12pbfxysk
title: "Task 8c: Restart replay from persisted cursor after selection change"
kind: task
status: closed
priority: 2
version: 4
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies:
  - type: blocks
    target: is-01kj3b52swpn9sdvjn68tbg4wp
parent_id: is-01kj3b52swpn9sdvjn68tbg4wp
created_at: 2026-02-22T19:01:23.293Z
updated_at: 2026-02-23T02:24:18.269Z
closed_at: 2026-02-23T02:24:18.264Z
close_reason: Implemented in commit f44fa13 with independent review APPROVE; validated via cargo test -p server --test receiver_selection_replay_interrupt_v11 and receiver_selection_backfill_v11.
---
Split from Task 8: cancel/restart behavior and interrupt regression tests.
