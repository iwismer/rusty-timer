---
type: is
id: is-01kj41ede233dehramy1w2bnnb
title: Rename 'Reset Epoch' button to 'Advance Epoch' in forwarder and server UIs
kind: bug
status: closed
priority: 3
version: 4
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies: []
parent_id: is-01kj3b514za6r2c96xxn5w3wcn
created_at: 2026-02-23T01:22:28.417Z
updated_at: 2026-02-23T02:24:18.959Z
closed_at: 2026-02-23T02:24:18.957Z
close_reason: Implemented in commit 93cb292 with independent UI review APPROVE; forwarder label updated to Advance Epoch and checks passed.
---
Rename 'Reset Epoch' button to 'Advance Epoch' in the forwarder UI only. The server UI stream details page reset epoch button is being replaced by a new 'Advance to Next Epoch' button (see rt-qf4g) and should be removed rather than renamed.
