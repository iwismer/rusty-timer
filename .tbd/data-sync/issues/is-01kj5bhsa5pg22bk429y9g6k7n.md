---
type: is
id: is-01kj5bhsa5pg22bk429y9g6k7n
title: "[Track E] sqlx offline cache not updated after migrations 0007/0008 — cargo sqlx prepare was not run; compile-time verification is stale"
kind: bug
status: closed
priority: 1
version: 5
labels:
  - blocking
dependencies: []
parent_id: is-01kj5b8y6cq49qakqd7ekv2hcj
created_at: 2026-02-23T13:38:19.076Z
updated_at: 2026-02-23T15:09:19.668Z
closed_at: 2026-02-23T15:09:19.660Z
close_reason: "sqlx prepare run against fresh Postgres 15 + all 8 migrations: 31 correct query files regenerated (identical to those already in HEAD), 17 stale files removed. cargo check -p server passes."
---
