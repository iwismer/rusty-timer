---
type: is
id: is-01kj45xf5bnwggnz1v5whfdq4k
title: Advance to Next Epoch button on stream details page is disabled by default and cannot be clicked
kind: bug
status: closed
priority: 1
version: 4
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies: []
parent_id: is-01kj3b514za6r2c96xxn5w3wcn
created_at: 2026-02-23T02:40:36.010Z
updated_at: 2026-02-23T03:01:36.136Z
closed_at: 2026-02-23T03:01:36.134Z
close_reason: "Fixed by commit 35167ed (advance action fallback by stream epoch). Independently reviewed and CI green on PR #89."
---
The 'Advance to Next Epoch' button on the stream details page appears disabled on load and cannot be clicked. Needs debugging — likely a conditional or reactive state issue causing the button to remain in a disabled state incorrectly.
