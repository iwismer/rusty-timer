---
type: is
id: is-01kj46sg9y6ekp9rhqad6rcaxp
title: Verify receiver UI available streams list auto-updates via SSE when new streams appear
kind: task
status: open
priority: 3
version: 2
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies: []
parent_id: is-01kj3b514za6r2c96xxn5w3wcn
created_at: 2026-02-23T02:55:54.685Z
updated_at: 2026-02-23T02:56:00.367Z
---
SSE infrastructure is already in place in the receiver UI — initSSE handles a 'streams_snapshot' event that updates the streams list (apps/receiver-ui/src/lib/sse.ts). Verify this actually covers the case of newly added streams appearing without a page refresh, or implement if not.
