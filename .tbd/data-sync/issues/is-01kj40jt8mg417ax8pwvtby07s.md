---
type: is
id: is-01kj40jt8mg417ax8pwvtby07s
title: Add human-readable name field to epochs on stream details page
kind: feature
status: open
priority: 3
version: 3
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies:
  - type: blocks
    target: is-01kj40yqaqz99zkhncgkqg5h29
parent_id: is-01kj3b514za6r2c96xxn5w3wcn
created_at: 2026-02-23T01:07:24.047Z
updated_at: 2026-02-23T01:14:02.142Z
---
Allow users to assign a human-readable name to each epoch, displayed in the epochs table on the stream details page. No existing backend support found — requires: (1) new DB column or table for epoch metadata, (2) new API endpoints to set/get epoch names, (3) update EpochInfo type in Rust backend to include name field, (4) UI to display and edit epoch names inline in the epochs table.
