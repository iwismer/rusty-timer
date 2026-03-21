# WS-Based Metrics & PR Review Fixes Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move stream metrics delivery from unauthenticated HTTP polling to the existing authenticated WS connection, and fix all issues found during PR review.

**Architecture:** Add a `ReceiverStreamMetrics` WS message to rt-protocol. The server pushes metrics after mode application and on live `DashboardEvent::MetricsUpdated` broadcasts. The receiver handles these in its session loop, replacing the HTTP-based `fetch_initial_metrics` and SSE `metrics_updated` handler. Other review fixes (SSE parser, epoch comparison, logging, frontend error surfacing) are independent tasks.

**Tech Stack:** Rust (rt-protocol, receiver, server), TypeScript/Svelte (receiver-ui), Tauri v2

---

### Task 1: Add `ReceiverStreamMetrics` to rt-protocol

**Files:**
- Modify: `crates/rt-protocol/src/lib.rs` (add struct + enum variant)

- [ ] **Step 1: Add the `ReceiverStreamMetrics` struct**

After the `ReceiverModeApplied` struct (around line 215), add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReceiverStreamMetrics {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub raw_count: i64,
    pub dedup_count: i64,
    pub retransmit_count: i64,
    pub lag_ms: Option<u64>,
    pub epoch_raw_count: i64,
    pub epoch_dedup_count: i64,
    pub epoch_retransmit_count: i64,
    pub epoch_lag_ms: Option<u64>,
    pub epoch_last_received_at: Option<String>,
    pub unique_chips: i64,
}
```

- [ ] **Step 2: Add the variant to the `WsMessage` enum**

In the `WsMessage` enum (around line 497), add after `ReceiverAck`:

```rust
ReceiverStreamMetrics(ReceiverStreamMetrics),
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p rt-protocol`
Expected: PASS (existing serde tests still work, new variant is just additive)

- [ ] **Step 4: Commit**

```bash
git add crates/rt-protocol/src/lib.rs
git commit -m "feat(rt-protocol): add ReceiverStreamMetrics WS message"
```

---

### Task 2: Server pushes initial metrics after mode application and forwards live updates

**Files:**
- Modify: `services/server/src/ws_receiver.rs` (add metrics push after `apply_mode`, subscribe to dashboard events)

**Structural change:** `apply_mode` currently returns `Result<ActiveMode, ...>`. Change it to return `Result<(ActiveMode, Vec<ResolvedStreamTarget>), ...>` so the caller has access to the resolved targets for both initial metrics push and live metrics filtering.

- [ ] **Step 1: Add helper function `send_stream_metrics`**

Add a helper near the top of `ws_receiver.rs` (after the `StreamSub` struct around line 52) that queries metrics for a single stream and sends a `ReceiverStreamMetrics` message:

```rust
async fn send_stream_metrics(
    socket: &mut WebSocket,
    state: &AppState,
    target: &ResolvedStreamTarget,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::repo::events::{count_unique_chips, fetch_stream_metrics};

    let metrics = match fetch_stream_metrics(&state.pool, target.stream_id).await? {
        Some(m) => m,
        None => return Ok(()), // No metrics row yet — skip silently
    };

    let unique_chips =
        count_unique_chips(&state.pool, target.stream_id, target.current_stream_epoch).await?;

    let msg = WsMessage::ReceiverStreamMetrics(rt_protocol::ReceiverStreamMetrics {
        forwarder_id: target.forwarder_id.clone(),
        reader_ip: target.reader_ip.clone(),
        raw_count: metrics.raw_count,
        dedup_count: metrics.dedup_count,
        retransmit_count: metrics.retransmit_count,
        lag_ms: metrics.lag_ms,
        epoch_raw_count: metrics.epoch_raw_count,
        epoch_dedup_count: metrics.epoch_dedup_count,
        epoch_retransmit_count: metrics.epoch_retransmit_count,
        epoch_lag_ms: metrics.epoch_lag_ms,
        epoch_last_received_at: metrics.epoch_last_received_at.map(|ts| ts.to_rfc3339()),
        unique_chips,
    });
    let json = serde_json::to_string(&msg)?;
    socket.send(Message::Text(json.into())).await?;
    Ok(())
}
```

- [ ] **Step 2: Change `apply_mode` return type to include resolved targets**

Change the `apply_mode` function signature from:

```rust
async fn apply_mode(...) -> Result<ActiveMode, Box<dyn std::error::Error + Send + Sync>>
```

to:

```rust
async fn apply_mode(...) -> Result<(ActiveMode, Vec<ResolvedStreamTarget>), Box<dyn std::error::Error + Send + Sync>>
```

**Live mode branch** (around line 529): Change `Ok(ActiveMode::Live)` to `Ok((ActiveMode::Live, targets))`. The `targets` variable from `resolve_live_targets` (line 490) is already in scope and has not been consumed — the loop at line 495 iterates `&targets`.

**Race mode branch** (around line 551): Change `apply_race_mode_forward_only` to also return its resolved targets. Currently it consumes `targets` in a `for target in targets` loop — change to `for target in &targets` to borrow instead of move, then return the targets. Change the return to `Ok((ActiveMode::Race { race_id, baseline }, race_targets))`.

**TargetedReplay branch** (around line 586): Return an empty vec: `Ok((ActiveMode::TargetedReplay, Vec::new()))`.

- [ ] **Step 3: Update `apply_mode` call site**

In `handle_receiver_socket` (around line 238), update the destructuring:

```rust
// FROM:
let mut active_mode = match apply_mode(...).await {
    Ok(mode) => mode,
// TO:
let (mut active_mode, resolved_targets) = match apply_mode(...).await {
    Ok(result) => result,
```

- [ ] **Step 4: Push initial metrics after mode application**

After `apply_mode` returns and before the main loop starts (around line 255), add:

```rust
// Push initial metrics for each subscribed stream.
for target in &resolved_targets {
    if let Err(e) = send_stream_metrics(&mut socket, &state, target).await {
        warn!(
            stream_id = %target.stream_id,
            error = %e,
            "failed to send initial stream metrics"
        );
    }
}
```

Note: TargetedReplay returns an empty vec, so no metrics are sent for replay-only sessions.

- [ ] **Step 5: Build `subscribed_stream_info` map for live metrics filtering**

After the initial metrics push, build the lookup map:

```rust
let mut subscribed_stream_info: HashMap<Uuid, (String, String)> = resolved_targets
    .iter()
    .map(|t| (t.stream_id, (t.forwarder_id.clone(), t.reader_ip.clone())))
    .collect();
```

- [ ] **Step 6: Subscribe to `dashboard_tx` and add select branch for live metrics**

Before the main loop, subscribe to the dashboard broadcast:

```rust
let mut dashboard_rx = state.dashboard_tx.subscribe();
```

Add `use crate::dashboard_events::DashboardEvent;` to the imports at the top of the file.

In the main `tokio::select!` loop (around line 351), add a new branch:

```rust
event = dashboard_rx.recv() => {
    match event {
        Ok(DashboardEvent::MetricsUpdated {
            stream_id,
            raw_count,
            dedup_count,
            retransmit_count,
            lag_ms,
            epoch_raw_count,
            epoch_dedup_count,
            epoch_retransmit_count,
            epoch_lag_ms,
            epoch_last_received_at,
            unique_chips,
            ..
        }) => {
            if let Some((forwarder_id, reader_ip)) = subscribed_stream_info.get(&stream_id) {
                let msg = WsMessage::ReceiverStreamMetrics(rt_protocol::ReceiverStreamMetrics {
                    forwarder_id: forwarder_id.clone(),
                    reader_ip: reader_ip.clone(),
                    raw_count,
                    dedup_count,
                    retransmit_count,
                    lag_ms,
                    epoch_raw_count,
                    epoch_dedup_count,
                    epoch_retransmit_count,
                    epoch_lag_ms,
                    epoch_last_received_at,
                    unique_chips,
                });
                if let Ok(json) = serde_json::to_string(&msg) {
                    if socket.send(Message::Text(json.into())).await.is_err() {
                        break;
                    }
                }
            }
        }
        Ok(_) => {} // Ignore non-metrics dashboard events
        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
            // Dashboard events are best-effort; skip lagged
        }
        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
    }
}
```

- [ ] **Step 7: Update `apply_race_refresh_forward_only` to also update `subscribed_stream_info`**

`apply_race_refresh_forward_only` adds/removes streams during race mode. It already mutates `subscriptions`. Similarly, it should update `subscribed_stream_info`.

Change `apply_race_refresh_forward_only` to accept `&mut HashMap<Uuid, (String, String)>` as an additional parameter. When new streams are added (subscribed), insert their `(forwarder_id, reader_ip)`. When streams are removed, remove them from the map.

Update the call site in the `race_refresh_interval.tick()` branch to pass `&mut subscribed_stream_info`.

- [ ] **Step 8: Run server tests**

Run: `cargo test -p rusty-timer-server`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add services/server/src/ws_receiver.rs
git commit -m "feat(server): push stream metrics over WS (initial + live updates)"
```

---

### Task 3: Receiver handles `ReceiverStreamMetrics` in session loop

**Files:**
- Modify: `services/receiver/src/session.rs` (add match arm)
- Modify: `services/receiver/src/ui_events.rs` (update `StreamMetricsPayload` to accept rt-protocol type directly)

- [ ] **Step 1: Add `from_ws` constructor to `StreamMetricsPayload`**

In `services/receiver/src/ui_events.rs`, add a new constructor that takes the rt-protocol struct directly, replacing the need for `UpstreamMetricsResponse`:

```rust
impl StreamMetricsPayload {
    pub fn from_ws(msg: &rt_protocol::ReceiverStreamMetrics) -> Self {
        Self {
            forwarder_id: msg.forwarder_id.clone(),
            reader_ip: msg.reader_ip.clone(),
            raw_count: msg.raw_count,
            dedup_count: msg.dedup_count,
            retransmit_count: msg.retransmit_count,
            lag_ms: msg.lag_ms,
            epoch_raw_count: msg.epoch_raw_count,
            epoch_dedup_count: msg.epoch_dedup_count,
            epoch_retransmit_count: msg.epoch_retransmit_count,
            unique_chips: msg.unique_chips,
            epoch_last_received_at: msg.epoch_last_received_at.clone(),
            epoch_lag_ms: msg.epoch_lag_ms,
        }
    }
}
```

- [ ] **Step 2: Add match arm in `run_session_loop`**

In `services/receiver/src/session.rs`, in the `match` on deserialized `WsMessage` (around line 116), add a new arm before the catch-all:

```rust
Ok(WsMessage::ReceiverStreamMetrics(metrics)) => {
    let payload = crate::ui_events::StreamMetricsPayload::from_ws(&metrics);
    let _ = deps.ui_tx.send(
        crate::ui_events::ReceiverUiEvent::StreamMetricsUpdated(payload),
    );
}
```

- [ ] **Step 3: Run receiver tests**

Run: `cargo test -p rusty-timer-receiver`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add services/receiver/src/session.rs services/receiver/src/ui_events.rs
git commit -m "feat(receiver): handle ReceiverStreamMetrics WS messages in session loop"
```

---

### Task 4: Remove HTTP-based metrics fetching from receiver

**Files:**
- Modify: `services/receiver/src/runtime.rs` (remove `fetch_initial_metrics`, `initial_metrics_entries`, `handle_metrics_updated`, `UpstreamMetricsPayload`, `should_emit_initial_metrics`, `spawn_initial_metrics_fetch`, metrics_updated SSE handling, and generation-based staleness logic from SSE refresher)
- Modify: `services/receiver/src/control_api.rs` (remove `fetch_stream_metrics`, `UpstreamMetricsResponse`, `StreamIdMap`, `build_stream_id_map`, `stream_id_map` from AppState, `dashboard_metrics_generation` and related methods)
- Modify: `services/receiver/src/ui_events.rs` (remove `from_upstream` method)

- [ ] **Step 1: Remove SSE `metrics_updated` handling from `consume_upstream_dashboard_events`**

In `runtime.rs`, in the `consume_upstream_dashboard_events` function (around line 899), remove the `metrics_updated` block:

```rust
// REMOVE this block:
if event_name == "metrics_updated"
    && let Some(data) = data
{
    handle_metrics_updated(state, &data).await;
}
```

- [ ] **Step 2: Remove `handle_metrics_updated` and `UpstreamMetricsPayload`**

Delete the `UpstreamMetricsPayload` struct (lines 585-603) and the `handle_metrics_updated` function (lines 605-645) from `runtime.rs`.

- [ ] **Step 3: Remove `fetch_initial_metrics`, `initial_metrics_entries`, `should_emit_initial_metrics`, `spawn_initial_metrics_fetch`**

Delete these functions from `runtime.rs`:
- `should_emit_initial_metrics` (lines 649-652)
- `initial_metrics_entries` (lines 654-679)
- `fetch_initial_metrics` (lines 681-749)
- `spawn_initial_metrics_fetch` (find and remove — it's a small wrapper)

- [ ] **Step 4: Remove generation-based metrics logic from SSE refresher**

In `run_upstream_dashboard_sse_refresher`, remove the `next_dashboard_metrics_generation()` and `spawn_initial_metrics_fetch()` calls (lines 836-837). Also remove the same pattern from `refresh_dashboard_snapshot_and_metrics` (line 553-555) — change it to just call `state.emit_streams_snapshot().await;` without the generation/spawn.

- [ ] **Step 5: Remove `fetch_stream_metrics`, `UpstreamMetricsResponse`, `StreamIdMap`, `build_stream_id_map` from control_api.rs**

Delete:
- `fetch_stream_metrics` function (lines 601-620)
- `UpstreamMetricsResponse` struct (lines 624-641)
- `build_stream_id_map` function (lines 644-654)
- `StreamIdMap` type alias (line 39)
- `stream_id_map` field from `AppState` struct (line 56)
- `stream_id_map` initialization from `AppState::with_integrity` (line 96)
- `UpstreamStreamInfo` struct and `ServerStreamsResponse` struct and `fetch_server_streams` function (if no longer used elsewhere — check first)

Note: `fetch_server_streams` is also used by `emit_streams_snapshot`. Keep it if still needed there. Only remove `fetch_stream_metrics` and the metrics-specific types.

- [ ] **Step 6: Remove `dashboard_metrics_generation` from AppState**

In `control_api.rs`, remove:
- `dashboard_metrics_generation` field from `AppState` (line 57)
- `dashboard_metrics_generation` initialization (line 97)
- `next_dashboard_metrics_generation` method (lines 125-129)
- `invalidate_dashboard_metrics_generation` method (lines 131-134)
- `current_dashboard_metrics_generation` method (lines 136-138)
- The call to `invalidate_dashboard_metrics_generation()` in `emit_connection_state_side_effects` (around line 177)

- [ ] **Step 7: Remove `from_upstream` from `StreamMetricsPayload`**

In `ui_events.rs`, remove the `from_upstream` method (keep `from_ws` added in Task 3).

- [ ] **Step 8: Update tests**

Remove or update tests that relied on the removed functions:
- Tests for `UpstreamMetricsPayload` deserialization in `runtime.rs`
- Tests for `fetch_initial_metrics` (the three staleness tests, the stream refresh test)
- Tests for `build_stream_id_map` in `tests/control_api.rs`
- Tests for `UpstreamMetricsResponse` deserialization

The `StreamMetricsPayload` serialization tests should stay (they test the UI event shape).

- [ ] **Step 9: Run all receiver tests**

Run: `cargo test -p rusty-timer-receiver`
Expected: PASS (with removed tests, remaining tests pass)

- [ ] **Step 10: Commit**

```bash
git add services/receiver/src/runtime.rs services/receiver/src/control_api.rs services/receiver/src/ui_events.rs services/receiver/tests/control_api.rs
git commit -m "refactor(receiver): remove HTTP-based metrics fetching in favor of WS delivery"
```

---

### Task 5: Fix SSE multi-line `data:` concatenation

**Files:**
- Modify: `services/receiver/src/runtime.rs` (`consume_sse_line_for_event`)

- [ ] **Step 1: Write a failing test**

Add a test in `runtime.rs` (in the existing `#[cfg(test)]` block near the SSE parser tests):

```rust
#[test]
fn sse_parser_concatenates_multiline_data() {
    let mut pending_event = None;
    let mut pending_data = None;
    assert!(consume_sse_line_for_event("event:test", &mut pending_event, &mut pending_data).is_none());
    assert!(consume_sse_line_for_event("data:line1", &mut pending_event, &mut pending_data).is_none());
    assert!(consume_sse_line_for_event("data:line2", &mut pending_event, &mut pending_data).is_none());
    let result = consume_sse_line_for_event("", &mut pending_event, &mut pending_data);
    assert_eq!(result, Some(("test".to_owned(), Some("line1\nline2".to_owned()))));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rusty-timer-receiver sse_parser_concatenates_multiline_data`
Expected: FAIL (currently overwrites data, so result would be `Some("line2")` not `Some("line1\nline2")`)

- [ ] **Step 3: Fix the parser**

In `consume_sse_line_for_event` (around line 579), change:

```rust
// FROM:
} else if let Some(rest) = line.strip_prefix("data:") {
    *pending_data = Some(rest.trim().to_owned());
}
```

```rust
// TO:
} else if let Some(rest) = line.strip_prefix("data:") {
    match pending_data {
        Some(existing) => {
            existing.push('\n');
            existing.push_str(rest.trim());
        }
        None => *pending_data = Some(rest.trim().to_owned()),
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rusty-timer-receiver sse_parser_concatenates_multiline_data`
Expected: PASS

- [ ] **Step 5: Run all SSE parser tests**

Run: `cargo test -p rusty-timer-receiver sse_parser`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add services/receiver/src/runtime.rs
git commit -m "fix(receiver): concatenate multi-line SSE data per spec instead of overwriting"
```

---

### Task 6: Fix epoch comparison edge case in frontend store

**Files:**
- Modify: `apps/receiver-ui/src/lib/store.svelte.ts`

- [ ] **Step 1: Write a failing test**

In `apps/receiver-ui/src/lib/store-updater.test.ts`, add a test:

```typescript
test("onStreamsSnapshot clears metrics for newly appearing streams", () => {
  // Pre-populate metrics for a stream
  const key = "fwd-new/10.0.0.1:10000";
  store.streamMetrics = new Map([
    [key, {
      forwarder_id: "fwd-new",
      reader_ip: "10.0.0.1:10000",
      raw_count: 100,
      dedup_count: 90,
      retransmit_count: 10,
      lag_ms: null,
      epoch_raw_count: 50,
      epoch_dedup_count: 45,
      epoch_retransmit_count: 5,
      epoch_lag_ms: null,
      epoch_last_received_at: null,
      unique_chips: 20,
    }],
  ]);
  // No previous streams (simulates first snapshot or stream re-appearing)
  store.streams = null;

  callbacks.onStreamsSnapshot({
    streams: [
      {
        forwarder_id: "fwd-new",
        reader_ip: "10.0.0.1:10000",
        stream_epoch: undefined,
      } as any,
    ],
    degraded: false,
    upstream_error: null,
  });

  expect(store.streamMetrics.has(key)).toBe(false);
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/receiver-ui && npx vitest run --reporter=verbose src/lib/store-updater.test.ts -t "newly appearing"`
Expected: FAIL (metrics not cleared because `undefined !== undefined` is `false`)

- [ ] **Step 3: Fix the epoch comparison**

In `store.svelte.ts`, in the `onStreamsSnapshot` callback (around line 894-898), change:

```typescript
// FROM:
for (const stream of s.streams) {
  const key = streamKey(stream.forwarder_id, stream.reader_ip);
  if (previousEpochByKey.get(key) !== stream.stream_epoch) {
    prunedMetrics.delete(key);
  }
}
```

```typescript
// TO:
for (const stream of s.streams) {
  const key = streamKey(stream.forwarder_id, stream.reader_ip);
  if (!previousEpochByKey.has(key) || previousEpochByKey.get(key) !== stream.stream_epoch) {
    prunedMetrics.delete(key);
  }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/receiver-ui && npx vitest run --reporter=verbose src/lib/store-updater.test.ts -t "newly appearing"`
Expected: PASS

- [ ] **Step 5: Run all store-updater tests**

Run: `cd apps/receiver-ui && npx vitest run --reporter=verbose src/lib/store-updater.test.ts`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add apps/receiver-ui/src/lib/store.svelte.ts apps/receiver-ui/src/lib/store-updater.test.ts
git commit -m "fix(receiver-ui): clear metrics for newly appearing streams in epoch comparison"
```

---

### Task 7: Improve logging and error surfacing

**Files:**
- Modify: `services/receiver/src/runtime.rs` (SSE refresher logging)
- Modify: `apps/receiver-ui/src/lib/store.svelte.ts` (initSSE error)

- [ ] **Step 1: Escalate SSE client creation failure to Error level**

In `runtime.rs`, in `run_upstream_dashboard_sse_refresher` (around line 777), change `UiLogLevel::Warn` to `UiLogLevel::Error`:

```rust
// FROM:
state.logger.log_at(
    UiLogLevel::Warn,
    format!("failed to create upstream SSE client: {e}"),
);
```

```rust
// TO:
state.logger.log_at(
    UiLogLevel::Error,
    format!("failed to create upstream SSE client: {e}"),
);
```

- [ ] **Step 2: Surface `initSSE` failure to store.error**

In `store.svelte.ts` (around line 928), change:

```typescript
// FROM:
initSSE({...})?.catch((e: unknown) => console.error("initSSE failed:", e));
```

```typescript
// TO:
initSSE({...})?.catch((e: unknown) => {
  console.error("initSSE failed:", e);
  store.error = `Event listener initialization failed: ${String(e)}`;
});
```

- [ ] **Step 3: Run frontend tests**

Run: `cd apps/receiver-ui && npx vitest run`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add services/receiver/src/runtime.rs apps/receiver-ui/src/lib/store.svelte.ts
git commit -m "fix(receiver): improve error surfacing for SSE client failure and initSSE"
```

---

### Task 8: Fix plan doc field name discrepancy

**Files:**
- Modify: `docs/superpowers/plans/2026-03-21-receiver-stream-metrics.md`

- [ ] **Step 1: Add a note at the top of the plan**

Add after the plan header:

```markdown
> **Note (2026-03-21):** The implementation uses `lag_ms` and `epoch_lag_ms` field names
> throughout the entire stack (Rust and TypeScript), preserving the unit suffix end-to-end.
> This differs from the original plan text which used `lag` and `epoch_lag`. See the design
> spec for the rationale.
```

- [ ] **Step 2: Commit**

```bash
git add docs/superpowers/plans/2026-03-21-receiver-stream-metrics.md
git commit -m "docs: add field name deviation note to metrics implementation plan"
```

---

### Task 9: Final verification

- [ ] **Step 1: Run all Rust tests**

Run: `cargo test --workspace`
Expected: All tests pass

- [ ] **Step 2: Run all frontend tests**

Run: `cd apps/receiver-ui && npx vitest run`
Expected: All tests pass

- [ ] **Step 3: Run clippy**

Run: `cargo clippy --workspace -- -D warnings`
Expected: No warnings

- [ ] **Step 4: Run svelte-check**

Run: `cd apps/receiver-ui && npx svelte-check`
Expected: No errors
