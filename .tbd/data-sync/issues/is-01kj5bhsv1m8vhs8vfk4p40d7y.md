---
type: is
id: is-01kj5bhsv1m8vhs8vfk4p40d7y
title: "[Track E] No rollback path documented for migration 0007 receiver_cursors PK change — manual SQL required for rollback"
kind: bug
status: closed
priority: 1
version: 5
labels:
  - blocking
dependencies: []
parent_id: is-01kj5b8y6cq49qakqd7ekv2hcj
created_at: 2026-02-23T13:38:19.614Z
updated_at: 2026-02-23T15:09:20.446Z
closed_at: 2026-02-23T15:09:20.441Z
close_reason: "Rollback SQL documented as comment block at top of migration 0007: DROP CONSTRAINT, ADD PRIMARY KEY (old columns), DROP INDEX, DROP TABLE — manual steps in reverse order."
---
