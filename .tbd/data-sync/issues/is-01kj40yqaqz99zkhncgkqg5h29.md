---
type: is
id: is-01kj40yqaqz99zkhncgkqg5h29
title: Replace stream epoch text field with dropdown in replay targets row when policy is 'replay'
kind: feature
status: open
priority: 2
version: 4
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies: []
parent_id: is-01kj3b514za6r2c96xxn5w3wcn
created_at: 2026-02-23T01:13:54.258Z
updated_at: 2026-02-23T01:41:35.900Z
---
When replay policy is 'replay', the stream epoch field in the replay targets row should be a dropdown instead of a free-text field. Each option should show: the epoch's human-readable name (if set, see rt-kddq) or fallback to epoch number + creation date/time. Each option should also indicate if the epoch is associated with a particular race.
