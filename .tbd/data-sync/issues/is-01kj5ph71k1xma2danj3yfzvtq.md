---
type: is
id: is-01kj5ph71k1xma2danj3yfzvtq
title: Add info-level tracing logs for race creation, participant upload, and bibchip upload in server service
kind: task
status: open
priority: 3
version: 1
labels: []
dependencies: []
created_at: 2026-02-23T16:50:14.705Z
updated_at: 2026-02-23T16:50:14.705Z
---

## Problem

Key operational events in the server service produce no structured log output on success. Race creation, participant (PPL) file upload, and bibchip file upload are all silent unless they fail. This makes it difficult to correlate server-side actions with observed behaviour during event-day operations.

## Scope

**In scope:** Add `info!()` tracing macros at the success paths for the three handlers listed below. Include relevant IDs and counts as structured fields.

**Out of scope:** Changes to error handling, UI logger (`state.logger.log`), or other services. Do not add logs to every handler — only the three explicitly requested.

## Handlers to Instrument

All in `services/server/src/http/races.rs`:

| Handler | Line range | Endpoint | Suggested log |
|---------|-----------|----------|---------------|
| `create_race` | 33-62 | `POST /api/v1/races` | `info!(race_id = %id, name = %name, "race created")` |
| `upload_participants` | 134-195 | `POST /api/v1/races/{race_id}/participants` | `info!(race_id = %race_id, count = %n, "participants uploaded")` |
| `upload_chips` | 197-236 | `POST /api/v1/races/{race_id}/chips` | `info!(race_id = %race_id, count = %n, "bibchips uploaded")` |

## Log Style Reference

Follow the existing pattern in `services/server/src/ws_receiver.rs`:
```rust
info!(device_id = %device_id, "receiver session ended");
```

Use `%` for Display-formatted values, `?` for Debug-formatted values.

## Acceptance Criteria

- [ ] `create_race` emits `info!()` on success with `race_id` and `name` fields.
- [ ] `upload_participants` emits `info!()` on success with `race_id` and participant count.
- [ ] `upload_chips` emits `info!()` on success with `race_id` and chip count.
- [ ] No changes to error paths or existing log lines.
- [ ] `cargo build` passes with no new warnings.

## Technical Notes

- `services/server/src/http/races.rs` is the only file to modify.
- Participant count: `parse_participant_bytes()` returns a `Vec`; use `.len()` for the count field.
- Chip count: `parse_bibchip_bytes()` returns a `Vec`; use `.len()`.
- No new dependencies needed; `tracing` is already imported in the crate.

## Validation

- `cargo test` passes.
- Run server locally, create a race, upload PPL and bibchip files, confirm log lines appear at INFO level in stdout.
