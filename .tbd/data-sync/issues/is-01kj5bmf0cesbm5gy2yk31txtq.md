---
type: is
id: is-01kj5bmf0cesbm5gy2yk31txtq
title: "PR #89 review summary: rt-1rb7 race/epoch selection"
kind: task
status: open
priority: 1
version: 2
labels: []
dependencies: []
created_at: 2026-02-23T13:39:46.826Z
updated_at: 2026-02-23T13:42:52.709Z
---
## PR #89 Review Summary — rt-1rb7 race/epoch selection and backfill (manual default)

**Overall recommendation: REQUEST CHANGES**

---

## Issues by Track and Severity

### BLOCKING (11 issues — must fix before merge)

| Bead | Track | Description |
|------|-------|-------------|
| rt-sg5v | C | Missing manual-default invariant test (receiver auto-select on startup) |
| rt-qagd | C | Missing 'no selection set' error path test |
| rt-4jsw | C | Missing invalid epoch ID rejection test |
| rt-bcn0 | D | status_http.rs:1862/1865 Content-Type inconsistency (error=text/plain should be application/json) |
| rt-nyj5 | D | status_http.rs:1721 percent-encoding validation rejects valid paths like 192.168.1.6%3A10000 |
| rt-x3uo | E | sqlx offline cache not regenerated after migrations 0007/0008 (cargo sqlx prepare not run) |
| rt-ndtw | E | Migration 0007 PK change lacks IF NOT EXISTS guards |
| rt-0dqn | E | No rollback path documented for migration 0007 receiver_cursors PK change |
| rt-r0ux | H | CSS classes text-semantic-ok/err undefined (should be text-status-ok/err) — feedback invisible |
| rt-1xqo | I | PR description claims 'No runtime/service code changes' — factually incorrect |
| rt-b69t | J | receiver-operations.md port confusion (:10001 vs :9090 for control API) |

### NON-BLOCKING (13 issues)

| Bead | Track | Description |
|------|-------|-------------|
| rt-fon5 | B/E | Migrations 0007/0008 CREATE TABLE lack IF NOT EXISTS guards |
| rt-5rhd | C | State transition edge cases untested (replace-active, disconnecting) |
| rt-auas | C | session_resume.rs minimal — missing multi-epoch and NULL field scenarios |
| rt-uyo7 | C | No test for SelectionApplied event broadcast after PUT /selection |
| rt-2gdk | D | health_endpoints.rs missing error path tests |
| rt-5hxk | F | Silent epoch loading failure — no user feedback when epoch fetch fails |
| rt-8qq3 | F | Missing AlertBanner test when putSelection fails |
| rt-9tuf | H | ReaderStatus missing current_epoch_name — blind editing in forwarder-ui |
| rt-y7nz | H | resetEpoch() new_epoch number discarded — no feedback to operator |
| rt-g5ra | I | rustfmt.toml edition=2024 compatibility with toolchain 1.93.1 unverified |
| rt-86eh | J | receiver/README.md missing GET/PUT /api/v1/selection in control API table |
| rt-0fan | J | No troubleshooting section for selection validation errors |
| rt-mpix | J | receiver-operations.md line 207 uses non-existent POST /api/v1/subscribe |

## Tracks that APPROVED

- Track A (Protocol & Contract): All 3 new message types correct; round-trip tests pass; WsMessage enum exhaustive. APPROVE.
- Track B (Receiver Service Rust): Manual-default invariant CONFIRMED SAFE in db.rs:49. No auto-select code path. NULL cursor handling correct. APPROVE.
- Track F (Receiver UI SvelteKit): Manual-default UI correct; all API endpoints aligned; retry-safe epoch loading confirmed. APPROVE.
- Track G (Server UI SvelteKit): All epoch name API shapes aligned; legacy reset-epoch removal intentional and correct. APPROVE.

## Acceptance Criteria Coverage (from rt-oi84)

- Manual-default invariant — implementation SAFE (Track B); test coverage MISSING (Track C: rt-sg5v)
- No-selection error path — NOT TESTED (Track C: rt-qagd)
- Invalid epoch rejection — NOT TESTED (Track C: rt-4jsw)
- Session resume with per-epoch cursors — PARTIAL (Track C: rt-auas)
- State transition guards — PARTIAL (Track C: rt-5rhd)
- SetSelection API — IMPLEMENTED AND TESTED
- SelectionApplied response — IMPLEMENTED; broadcast NOT TESTED (Track C: rt-uyo7)
- Operator runbook documentation — MOSTLY DONE; port error (Track J: rt-b69t)
