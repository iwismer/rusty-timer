---
type: is
id: is-01kj2zsh4rs03byre73dwf7y8v
title: Design v1.1 receiver race/epoch selection and backfill (manual default)
kind: feature
status: closed
priority: 2
version: 5
labels: []
dependencies: []
created_at: 2026-02-22T15:34:21.072Z
updated_at: 2026-02-22T21:42:20.545Z
closed_at: 2026-02-22T21:42:20.543Z
close_reason: "All Task 5/6/7/10 scope implemented, independently reviewed, integrated, pushed, and PR #89 CI is green (run 22285857575)."
---
Design activity for v1.1 race/epoch selection and backfill with product decision: receiver default selection is manual; race/current + resume remains opt-in.

## Notes

2026-02-22: committed docs/plans de-tracking on branch iwismer/setup-rusty-timer (commit e0f9ea6). Opened PR #89 (https://github.com/iwismer/rusty-timer/pull/89) titled for rt-1rb7 with manual-default framing. gh pr checks --watch reported no checks configured for this branch.
