---
type: is
id: is-01kj46h4x0zt8f4g4xdrfz9n33
title: Race mode with 'current' replay policy fails to receive events on fresh start (no existing cursor)
kind: bug
status: closed
priority: 1
version: 5
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies: []
parent_id: is-01kj3b514za6r2c96xxn5w3wcn
created_at: 2026-02-23T02:51:20.863Z
updated_at: 2026-02-23T03:18:19.292Z
closed_at: 2026-02-23T03:18:19.286Z
close_reason: "Fixed by commit f26099f: Resume+Current no-cursor start-cursor logic updated, stale-epoch/live behavior preserved, and regression tests added. Reviewed independently and CI checks passed on PR #89."
---
When receiver is configured with race mode, a race set, and replay policy 'current', no events are received on a fresh start. Setting epoch scope to 'all' instead of 'current' does work correctly. Workaround: switching to manual mode (which receives events), then switching back to race mode also restores event reception. Suspected cause: race mode may not correctly initialise a cursor when none exists yet and epoch scope is 'current'. Needs investigation — root cause could be elsewhere.
