---
type: is
id: is-01kj5avabbs4ysb749bqc9gctj
title: Rename epoch scope dropdown option 'current' to 'current and future' in receiver UI
kind: task
status: closed
priority: 3
version: 5
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels:
  - deferred
dependencies: []
parent_id: is-01kj3b514za6r2c96xxn5w3wcn
created_at: 2026-02-23T13:26:02.857Z
updated_at: 2026-02-23T18:56:22.470Z
closed_at: 2026-02-23T18:56:22.468Z
close_reason: "Fixed in PR #98"
---
Rename the 'current' option in the epoch scope dropdown to 'current and future', as this better reflects the actual behaviour. Before making the change, verify that the underlying behaviour does indeed include future epochs (i.e. the receiver continues receiving events when the epoch advances), and update the label accordingly. Also update any related plain-language description added for this option (see rt-r0bp).
