---
type: is
id: is-01kj43fhxcghk0vjk7jqa20mmv
title: Add receiver admin page to reset cursors on a per-stream basis
kind: feature
status: closed
priority: 2
version: 4
spec_path: docs/plans/2026-02-22-race-epoch-receiver-v1.1-design.md
labels: []
dependencies: []
parent_id: is-01kj3b514za6r2c96xxn5w3wcn
created_at: 2026-02-23T01:58:02.921Z
updated_at: 2026-02-23T04:44:29.407Z
closed_at: 2026-02-23T04:44:29.405Z
close_reason: Implemented receiver admin per-stream cursor reset UI/API with guard, tests, CI remediation, and review approvals; merged in commits a78a0c2/08e7be9.
---
Add an admin page to the receiver UI (modelled after the server UI admin page) that allows resetting cursors on a per-stream basis. Should list streams and provide a reset cursor action for each.
