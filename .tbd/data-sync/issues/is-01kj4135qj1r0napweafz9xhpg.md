---
type: is
id: is-01kj4135qj1r0napweafz9xhpg
title: Epoch race mapping table should update in real-time via SSE when epoch is reset (no page refresh required)
kind: feature
status: closed
priority: 2
version: 4
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies: []
parent_id: is-01kj3b514za6r2c96xxn5w3wcn
created_at: 2026-02-23T01:16:20.077Z
updated_at: 2026-02-23T04:44:29.228Z
closed_at: 2026-02-23T04:44:29.226Z
close_reason: Implemented SSE-based epoch mapping auto-refresh with coverage and review approvals; merged in commits a78a0c2/08e7be9.
---
The epoch race mapping table on the stream details page should refresh automatically when an epoch reset occurs, without requiring a manual page reload. SSE infrastructure already exists in server-ui (apps/server-ui/src/lib/sse.ts) — wire up an epoch-reset event to trigger a table reload.
