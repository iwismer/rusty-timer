# Code Review Findings — Remote Forwarding Suite

**Date:** 2026-02-17
**Branch:** `polish/repo-cleanup-and-review`
**Scope:** Full system review — protocol crates, services, frontend, integration tests, deployment/ops

Five parallel reviews were conducted by independent agents:

| Area | Reviewer | Rating |
|------|----------|--------|
| A — Protocol & core crates | Explore agent | 7/10 |
| B — Forwarder & Server services | Explore agent | 6/10 |
| C — Receiver, Streamer, Emulator | Explore agent | 5/10 |
| D — Frontend & integration tests | Explore agent | 6.5/10 |
| E — System architecture & ops | Explore agent | 7/10 |

---

## CRITICAL ISSUES

These must be fixed before deploying to any network-exposed environment.

---

### C-1: Receiver `main.rs` Never Starts the Service

**Severity:** CRITICAL — the receiver binary is non-functional
**Source:** Review C
**File:** `services/receiver/src/main.rs`

The `main` function parses config and initialises logging but never starts the upstream
session, the receiver proxy, or the control HTTP server. The binary exits immediately.

**Fix:** Wire up the three async tasks (upstream session, proxy fanout, control API) and
`.await` a `tokio::join!` so they all run.

---

### C-2: Lagged Proxy Clients Silently Drop Events

**Severity:** CRITICAL — data loss in receiver
**Source:** Review C
**File:** `services/receiver/src/proxy.rs`

When a downstream subscriber's channel is full the server uses `try_send`, discarding the
event without logging or metrics. A slow client (e.g. lagged display) will silently miss
reads with no indication in logs.

**Fix:** Log the drop at `warn!` level with the subscriber ID and lag count. Consider a
configurable high-water-mark that disconnects hopelessly-lagged clients explicitly.

---

### C-3: Unsafe Cursor ACK Pattern in Receiver

**Severity:** CRITICAL — can acknowledge reads that were never delivered
**Source:** Review C
**File:** `services/receiver/src/session.rs`

The cursor is advanced before confirming delivery to the local SQLite journal. A crash
between the ACK and the write means those events are permanently lost.

**Fix:** Write to the local journal first, then advance the cursor.

---

### B-1: Race Condition in First-Connection-Wins Logic

**Severity:** CRITICAL — two forwarders for the same device can both be accepted
**Source:** Review B
**File:** `services/server/src/ws_forwarder.rs`

The duplicate-connection check reads the in-memory connection map and then inserts into it
in two separate steps. Two connection requests arriving simultaneously can both read an
empty slot and both proceed.

**Fix:** Hold the map lock across the read-then-insert, or use an atomic compare-and-swap.

---

### B-2: Token Comparison Is Not Constant-Time

**Severity:** CRITICAL — timing side-channel leaks token bytes
**Source:** Reviews B and E
**File:** `services/server/src/auth.rs`

The `WHERE token_hash = $1` SQL equality is not constant-time. An attacker who can measure
response latency can recover valid token hashes bit-by-bit.

**Fix:** Fetch all candidate rows matching `device_id` and compare hashes in application
code using `subtle::ConstantTimeEq`.

---

### E-1: All HTTP API Endpoints Are Unauthenticated

**Severity:** CRITICAL — full data exfiltration and administrative control open to any client
**Source:** Review E
**Files:** `services/server/src/http/streams.rs`, `services/server/src/lib.rs`

Every dashboard API endpoint (`GET /api/v1/streams`, `PATCH`, `GET …/metrics`,
`GET …/export.*`, `POST …/reset-epoch`) accepts requests from any network client without
any bearer token check. The WebSocket ingest endpoint requires authentication, but the HTTP
plane does not.

**Fix (short-term):** Mark the server as internal-only and document it must not be exposed
to untrusted networks without a reverse proxy with auth. Add `Authorization: Bearer` checks
at minimum to mutating endpoints (`PATCH`, `POST reset-epoch`).

**Fix (long-term):** Add Axum middleware that validates a bearer token on all
`/api/v1/*` routes.

---

### E-2: `/readyz` Returns Static "ok" — Never Checks the Database

**Severity:** CRITICAL — container orchestrators report ready when DB is unreachable
**Source:** Review E
**File:** `services/server/src/lib.rs`

`GET /readyz` always returns 200 with body `"ok"`. The runbook explicitly states it should
confirm database connectivity. Kubernetes/Docker health checks will route traffic to a server
that cannot serve requests.

**Fix:**

```rust
pub async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    match sqlx::query("SELECT 1").execute(&state.pool).await {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE,
    }
}
```

---

### B-3: Event Loss on Journal Write Failure

**Severity:** CRITICAL — ACK sent to peer after failed local write
**Source:** Review B
**File:** `services/forwarder/src/journal.rs`

After a batch of events is received over the network, the journal write is attempted and —
if it fails — the batch is still ACKed to the upstream server. Those events are permanently
lost from the forwarder's replay buffer.

**Fix:** Only send the ACK after a successful `fsync`/`flush` of the journal. On write
failure, close the connection and let the server retransmit.

---

### A-1: Panics in Production Code Paths

**Severity:** CRITICAL — process crash on bad input
**Source:** Review A
**Files:** Various `unwrap()` calls in `crates/rt-protocol`, `crates/ipico-core`

Several `unwrap()` calls are in paths reachable from untrusted network input (message
deserialization, IPICO frame parsing). A malformed packet can panic the entire Tokio runtime.

**Fix:** Replace `unwrap()` with `?` or explicit error handling in any function reachable
from socket input. Reserve `expect("invariant: …")` for truly programmer-error-only paths
with a comment explaining the invariant.

---

## IMPORTANT ISSUES

These should be fixed before declaring the system production-ready.

---

### I-1: Receiver Cursor Resume Ignores Epoch Reset

**Severity:** Important
**Source:** Review C
**File:** `services/receiver/src/session.rs`

When the server sends a `StreamReset` message (epoch change), the receiver resumes its
cursor from the old epoch's position. It will either re-receive duplicates or miss the new
stream's early events.

**Fix:** On `StreamReset`, reset the cursor to `(new_epoch, seq=0)`.

---

### I-2: Partial Batch Save Followed by Full ACK

**Severity:** Important
**Source:** Review B
**File:** `services/server/src/ingest.rs`

The server saves events from a batch one-by-one without a transaction. If the database
rejects event N in a batch of M, events 1…N-1 are saved but the ACK is not sent (an error
is returned). However, on the next connect the forwarder retransmits the whole batch and
events 1…N-1 are inserted again. The deduplication key prevents double-counting but the
code path relies on this silently.

**Fix:** Wrap the batch insert in a single PostgreSQL transaction so it is atomic.

---

### I-3: No Reconnect Backoff in Receiver or Forwarder

**Severity:** Important
**Source:** Review C
**Files:** `services/receiver/src/session.rs`, `services/forwarder/src/uplink.rs`

Both services reconnect immediately on connection loss, with no exponential backoff or
jitter. Under a server outage this causes a thundering-herd of simultaneous reconnects.

**Fix:** Add exponential backoff (1 s → 2 s → 4 s … cap at 60 s) with ±20 % jitter.

---

### I-4: `#[serde(default)]` Silently Omits Required Fields

**Severity:** Important
**Source:** Review A
**Files:** `crates/rt-protocol/src/lib.rs`

Several `WsMessage` variants use `#[serde(default)]` on fields that are semantically
required (e.g. `seq`, `stream_epoch`). A message from an old client that omits these fields
will deserialize successfully with a zero default rather than failing with an error.

**Fix:** Remove `#[serde(default)]` from required fields. Use `Option<T>` explicitly where
true optionality is intended so the caller handles the `None` case.

---

### I-5: Dashboard Detail Page Double-Loads Data on Mount

**Severity:** Important — race condition between reactive block and `onMount`
**Source:** Review D
**File:** `apps/server-ui/src/routes/streams/[streamId]/+page.svelte`

Both `onMount(() => loadData(streamId))` and `$: if (streamId) { loadData(streamId); }` are
present. On first render both fire, causing two parallel in-flight requests. If the user
navigates quickly between streams the responses can arrive out of order.

**Fix:** Remove the `onMount` block; the reactive `$:` statement is sufficient for
SvelteKit route parameters.

---

### I-6: Receiver-UI `connect`/`disconnect` Bypass the Shared `apiFetch` Wrapper

**Severity:** Important
**Source:** Review D
**File:** `apps/receiver-ui/src/lib/api.ts`

`connect()` and `disconnect()` call `fetch()` directly, checking `resp.status !== 200 &&
resp.status !== 202`. They do not call `resp.text()` to include the server message in the
thrown error. All other functions use the `apiFetch` wrapper which does this correctly.

**Fix:** Refactor to use `apiFetch` (treating 202 as success is fine via a custom status
check), so error messages from the receiver control API are surfaced to the user.

---

### I-7: `uwrap` in Receiver Subscription Handler

**Severity:** Important
**Source:** Review B
**File:** `services/server/src/fanout.rs`

The code that broadcasts events to receiver subscriptions calls `.unwrap()` on the send
result. If a receiver's channel is closed (client disconnected) this panics the broadcast
task for all receivers.

**Fix:** Use `if let Err(e) = sender.send(…)` and log a warning; remove the disconnected
subscriber from the map.

---

### I-8: Power-Loss Tests Do Not Exercise Real SIGKILL

**Severity:** Important — durability claims are unverified against true power loss
**Source:** Review D
**File:** `tests/integration/power_loss_forwarder.rs`

The tests simulate journal durability by dropping the `Journal` struct. This validates WAL
semantics but does not test recovery from an actual SIGKILL during an in-flight write.

**Recommendation:** Add a separate test that spawns the forwarder binary as a child process
(`std::process::Command`), sends `SIGKILL`, and verifies the journal survives and replays
correctly on restart.

---

### I-9: Missing IPICO Timestamp Validation

**Severity:** Important
**Source:** Review A
**File:** `crates/ipico-core/src/parser.rs`

The parser accepts any timestamp from the IPICO reader without range-checking. A reader
with a wrong clock can inject events far in the past or future; the server stores them
without warning.

**Fix:** Reject timestamps more than (e.g.) 24 hours from server wall time and log a
`warn!`.

---

### I-10: Forwarder Upstream URL Not Persisted — Lost After Restart

**Severity:** Important
**Source:** Review C
**File:** `services/forwarder/src/config.rs`

If the forwarder is reconfigured to point at a new server URL at runtime, this change is
not persisted to the TOML file and is lost on restart.

**Fix:** Either document this limitation clearly ("runtime URL changes are ephemeral; edit
`/etc/rusty-timer/forwarder.toml`") or persist changes to the config file.

---

### I-11: No Prometheus/OpenMetrics Export

**Severity:** Important — no production monitoring capability
**Source:** Review E
**File:** `services/server/src/lib.rs`

The server collects metrics in the `stream_metrics` table but exposes no machine-readable
metrics endpoint. There is no way to set up Prometheus alerts on event lag, connection
count, or error rate.

**Suggested metrics to expose on `GET /metrics`:**
- `rt_ws_connections_active` (gauge)
- `rt_events_received_total` (counter, labels: forwarder_id)
- `rt_events_deduplicated_total` (counter)
- `rt_receiver_backlog_events` (gauge, labels: stream_id)
- `rt_event_lag_milliseconds` (gauge, labels: stream_id)

---

### I-12: No CORS Headers — Dashboard Cannot Be Served from a Different Origin

**Severity:** Important (blocks common deployment patterns)
**Source:** Review E
**File:** `services/server/src/lib.rs`

There is no `tower_http::cors` layer on the Axum router. The embedded dashboard works (same
origin), but any deployment that puts the dashboard behind a CDN or separate domain will hit
CORS errors.

**Fix:** Add an explicit `CorsLayer` allowing the configured dashboard origin. Even for
same-origin deployments, an explicit deny-by-default CORS policy is good hygiene.

---

### I-13: Systemd Unit Lacks Key Security Directives

**Severity:** Important
**Source:** Review E
**File:** `deploy/systemd/rt-forwarder.service`

The unit has `NoNewPrivileges=yes` and `ProtectSystem=strict`, but is missing:
`PrivateTmp=yes`, `PrivateDevices=yes`, `MemoryDenyWriteExecute=yes`,
`RestrictNamespaces=yes`, `LockPersonality=yes`, `RestrictSUIDSGID=yes`,
`LimitNOFILE=65536`, `LimitNPROC=512`, resource accounting (`CPUQuota=`).

**Fix:** Add the missing directives. Run `systemd-analyze security rt-forwarder.service`
to verify the hardening score.

---

### I-14: Integration Test Servers Are Not Cancelled on Test Failure

**Severity:** Important — test pollution and memory leaks on failure
**Source:** Review D
**Files:** `tests/integration/*.rs`

In-process server tasks are spawned via `tokio::spawn` but their `JoinHandle`s are not
stored. On test failure, these tasks continue running during subsequent tests, potentially
processing requests meant for a different test.

**Fix:** Store `JoinHandle`s in a guard struct that calls `handle.abort()` on `Drop`.

---

## MINOR ISSUES

---

### M-1: `ApiError` Interface Exported but Never Used

**File:** `apps/server-ui/src/lib/api.ts:32`
The `ApiError` interface is exported but errors are always coerced to strings. Either use
it for structured error handling or remove it to reduce dead exports.

---

### M-2: `MockWsServer` Does Not Send Heartbeat Messages

**File:** `crates/rt-test-utils/src/lib.rs`
Tests using `MockWsServer` never receive `Ping` messages, so reconnect-on-timeout paths
are untested.

---

### M-3: `TimingReader` Reuses the Read Buffer Across Iterations

**File:** `crates/timer-core/src/workers/timing_reader.rs`
The buffer is cleared with `buf.clear()` but not re-allocated. On a very long run the
Vec capacity grows unbounded.

---

### M-4: Missing `.env.example` Referenced in Deployment Docs

**File:** `docs/docker-deployment.md`
The guide instructs `cp deploy/.env.example deploy/.env` but the file does not exist.
Create it with commented-out defaults for all required variables.

---

### M-5: Token Provisioning SQL Not Documented

**Files:** `docs/runbooks/server-operations.md`
The runbook tells operators to "contact the system operator for token provisioning" instead
of providing the exact SQL. Add a ready-to-run `INSERT INTO device_tokens` template.

---

### M-6: Dashboard API Fetch Has No Request Timeout

**Files:** `apps/server-ui/src/lib/api.ts`, `apps/receiver-ui/src/lib/api.ts`
A hung server causes the UI to freeze indefinitely. Wrap `fetch` calls with an
`AbortController` and a 30-second timeout.

---

### M-7: Receiver UI Has No Status Polling

**File:** `apps/receiver-ui/src/routes/+page.svelte`
Connection status is loaded once on mount and never refreshed. Add a polling interval
(2–5 s) so the UI reflects external state changes.

---

### M-8: Svelte Config `vitePreprocess` Import Differs Between Apps

Dashboard imports from `@sveltejs/vite-plugin-svelte`; receiver-ui imports from
`@sveltejs/kit/vite`. Both work, but standardise on one import source.

---

### M-9: `String` Used for Strongly-Typed IDs

**Source:** Review A
**Files:** Various
Fields like `forwarder_id`, `device_id`, `stream_id` are all `String` in various structs.
Using newtype wrappers (`struct ForwarderId(String)`) would catch transposed ID arguments at
compile time.

---

### M-10: `reset-epoch` Runbook Shows Auth Header, Implementation Ignores It

**Files:** `docs/runbooks/server-operations.md`, `services/server/src/http/streams.rs`
The runbook example passes `-H "Authorization: Bearer <admin-token>"` but the endpoint
ignores it. Fix the implementation (see C-1 above) so the runbook is accurate.

---

## TEST COVERAGE GAPS

| Area | Gap | Priority |
|------|-----|----------|
| Dashboard API | No tests for 5xx / 404 error paths | Medium |
| Receiver-UI API | No tests for `connect`/`disconnect` error responses | Medium |
| Chaos tests | `raw_count`, `dedup_count` in `stream_metrics` never asserted | Medium |
| Forwarder journal | No real-SIGKILL test; drop-based tests only | High |
| IPICO parser | No tests for invalid / truncated frames | Medium |
| Protocol | No negative-path tests for unknown message kinds | Low |
| Receiver session | No test for cursor resume after epoch reset | High |
| Server token auth | No test for timing-attack mitigation once fixed | High |

---

## TEST RUNABILITY SUMMARY

| Suite | Command | Requires |
|-------|---------|---------|
| Rust unit + lib tests | `cargo test --lib --workspace` | Nothing |
| Rust all tests (excl. integration) | `cargo test --lib --workspace --bins` | Nothing |
| Integration tests | `cargo test --test <name> -- --test-threads=4` | Docker daemon |
| Dashboard unit tests | `cd apps/server-ui && npm test` | Node |
| Receiver-UI unit tests | `cd apps/receiver-ui && npm test` | Node |
| Dashboard E2E | `cd apps/server-ui && npm run build && npm run test:e2e` | Node + Playwright browsers |

All Rust unit and JavaScript unit tests can be run by an agent with no external
dependencies. Integration tests require Docker. Dashboard E2E requires Playwright browser
binaries (`npx playwright install`).

---

## OVERALL ASSESSMENT

| Component | Rating | Notes |
|-----------|--------|-------|
| Protocol & core crates | 7/10 | Good type safety; serde defaults and panics are risks |
| Forwarder service | 7/10 | Solid journaling; race on first-connection-wins |
| Server service | 6/10 | Unauthenticated HTTP API is a blocking issue |
| Receiver service | 4/10 | main.rs is non-functional; cursor ACK unsafe |
| Streamer service | 7/10 | Simple and clean |
| Emulator service | 7/10 | Good determinism; port-collision not checked |
| Dashboard frontend | 7/10 | Clean UI; double-load race; no timeout |
| Receiver UI frontend | 6/10 | connect/disconnect inconsistent error handling |
| Integration tests | 7/10 | Excellent coverage; cleanup handles missing |
| Deployment & ops | 6/10 | Great docs; auth gaps; /readyz not real |
| **Overall** | **6.5/10** | Functional core; several critical issues to resolve |

**Recommendation:** Fix C-1 (non-functional receiver) and E-1 (unauthenticated HTTP API)
before any network-exposed deployment. Fix B-1 (race condition), B-2/E-2 (timing attack,
/readyz), and C-2/C-3 (proxy drops, cursor ACK) before production. The system is
architecturally sound and well-documented; closing these gaps brings it to production grade.
