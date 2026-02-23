---
type: is
id: is-01kj5bhsjxmgm62yc3n24n63nc
title: "[Track E] Migration 0007 receiver_cursors PRIMARY KEY change lacks idempotency guards — no IF EXISTS/IF NOT EXISTS; re-running fails"
kind: bug
status: closed
priority: 1
version: 5
labels:
  - blocking
dependencies: []
parent_id: is-01kj5b8y6cq49qakqd7ekv2hcj
created_at: 2026-02-23T13:38:19.355Z
updated_at: 2026-02-23T15:09:20.017Z
closed_at: 2026-02-23T15:09:20.012Z
close_reason: "Migration 0007 updated: CREATE TABLE → IF NOT EXISTS; ALTER TABLE DROP CONSTRAINT wrapped in DO $$ EXCEPTION block (idempotent); ADD PRIMARY KEY wrapped in DO $$ EXCEPTION block (idempotent)."
---
