# Receiver Stream Metrics Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add server-sourced stream metrics (raw/dedup/retransmit counts, lag, unique chips, last read time) to the receiver's expanded stream row, fed by the server's existing SSE stream and HTTP metrics API.

**Architecture:** The receiver's Rust backend already subscribes to the server's SSE endpoint at `/api/v1/events` (in `runtime.rs:564`). We extend this existing SSE consumer to also parse `data:` lines for `metrics_updated` events, resolve the server's `stream_id` UUID to a `forwarder_id/reader_ip` key, and emit a new `StreamMetricsUpdated` Tauri event. The frontend stores metrics in a reactive map and renders them in the expanded stream row. An initial HTTP fetch fills metrics on connection.

**Tech Stack:** Rust (receiver library crate), Svelte 5 + TypeScript (receiver-ui), Tauri v2 IPC events, reqwest HTTP client, serde_json

**Spec:** `docs/superpowers/specs/2026-03-21-receiver-stream-metrics-design.md`

---

> **Note (2026-03-21):** The implementation uses `lag_ms` and `epoch_lag_ms` field names
> throughout the entire stack (Rust and TypeScript), preserving the unit suffix end-to-end.
> This differs from the original plan text which used `lag` and `epoch_lag`. See the design
> spec for the rationale.

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `services/receiver/src/ui_events.rs` | Add `StreamMetricsUpdated` variant + `StreamMetricsPayload` struct |
| Modify | `services/receiver/src/runtime.rs` | Extend SSE parser to capture `data:` lines; handle `metrics_updated` events; add initial metrics fetch; add stream_id→key map |
| Modify | `services/receiver/src/control_api.rs` | Add `fetch_stream_metrics` HTTP helper; expose `UpstreamStreamInfo.stream_id` for mapping |
| Modify | `apps/receiver-ui/src-tauri/src/main.rs` | Add `stream_metrics_updated` to event bridge |
| Modify | `apps/receiver-ui/src/lib/api.ts` | Add `StreamMetrics` TypeScript interface |
| Modify | `apps/receiver-ui/src/lib/sse.ts` | Add `stream_metrics_updated` event listener |
| Modify | `apps/receiver-ui/src/lib/store.svelte.ts` | Add `streamMetrics` map, update function, SSE callback |
| Modify | `apps/receiver-ui/src/lib/components/StreamsTab.svelte` | Render metrics in expanded row, live timer, help text |
| Modify | `services/receiver/tests/control_api.rs` | Test for `fetch_stream_metrics` |
| Modify | `apps/receiver-ui/src/lib/components/StreamsTab.test.ts` | Test metrics rendering in expanded row |

---

## Task 1: Add `StreamMetricsUpdated` Event to Receiver Backend

**Files:**
- Modify: `services/receiver/src/ui_events.rs:39-63`

- [ ] **Step 1: Write the test**

Add to `services/receiver/src/ui_events.rs` at the bottom (or `services/receiver/tests/` if a test file exists for ui_events). Since this is a simple struct + enum variant, we'll test serialization:

In `services/receiver/src/ui_events.rs`, add a `#[cfg(test)]` module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_metrics_updated_serializes_with_correct_type_tag() {
        let event = ReceiverUiEvent::StreamMetricsUpdated(StreamMetricsPayload {
            forwarder_id: "fwd-1".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            raw_count: 100,
            dedup_count: 80,
            retransmit_count: 20,
            lag: Some(1500),
            epoch_raw_count: 50,
            epoch_dedup_count: 40,
            epoch_retransmit_count: 10,
            unique_chips: 30,
            epoch_last_received_at: Some("2026-03-21T12:00:00Z".to_owned()),
            epoch_lag: Some(500),
        });
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "stream_metrics_updated");
        assert_eq!(json["forwarder_id"], "fwd-1");
        assert_eq!(json["raw_count"], 100);
        assert_eq!(json["lag"], 1500);
        assert_eq!(json["unique_chips"], 30);
    }

    #[test]
    fn stream_metrics_updated_serializes_null_lag() {
        let event = ReceiverUiEvent::StreamMetricsUpdated(StreamMetricsPayload {
            forwarder_id: "fwd-1".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            raw_count: 0,
            dedup_count: 0,
            retransmit_count: 0,
            lag: None,
            epoch_raw_count: 0,
            epoch_dedup_count: 0,
            epoch_retransmit_count: 0,
            unique_chips: 0,
            epoch_last_received_at: None,
            epoch_lag: None,
        });
        let json = serde_json::to_value(&event).unwrap();
        assert!(json["lag"].is_null());
        assert!(json["epoch_last_received_at"].is_null());
        assert!(json["epoch_lag"].is_null());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver --lib ui_events::tests 2>&1`
Expected: FAIL — `StreamMetricsUpdated` variant and `StreamMetricsPayload` struct don't exist yet.

- [ ] **Step 3: Add the struct and enum variant**

In `services/receiver/src/ui_events.rs`, add the payload struct before the `ReceiverUiEvent` enum:

```rust
#[derive(Clone, Debug, Serialize)]
pub struct StreamMetricsPayload {
    pub forwarder_id: String,
    pub reader_ip: String,
    pub raw_count: i64,
    pub dedup_count: i64,
    pub retransmit_count: i64,
    pub lag: Option<u64>,
    pub epoch_raw_count: i64,
    pub epoch_dedup_count: i64,
    pub epoch_retransmit_count: i64,
    pub unique_chips: i64,
    pub epoch_last_received_at: Option<String>,
    pub epoch_lag: Option<u64>,
}
```

Add a new variant to the `ReceiverUiEvent` enum (after `LastRead`):

```rust
    StreamMetricsUpdated(StreamMetricsPayload),
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver --lib ui_events::tests 2>&1`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add services/receiver/src/ui_events.rs
git commit -m "feat(receiver): add StreamMetricsUpdated event variant"
```

---

## Task 2: Wire Event Through Tauri Bridge

**Files:**
- Modify: `apps/receiver-ui/src-tauri/src/main.rs:34-44` (the `ui_event_name` function)

- [ ] **Step 1: Add the event name mapping**

In `apps/receiver-ui/src-tauri/src/main.rs`, find the `ui_event_name` function (around line 34) which maps `ReceiverUiEvent` variants to Tauri event name strings. Add a new arm:

```rust
ReceiverUiEvent::StreamMetricsUpdated(_) => "stream_metrics_updated",
```

This should be added after the existing `LastRead` arm. The existing `bridge_action_from_item` function already handles all non-Resync variants via `BridgeAction::EmitEvent`, so no changes needed there.

- [ ] **Step 2: Verify compilation**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo check -p receiver-ui 2>&1`
Expected: compiles without errors. If there's a non-exhaustive match warning/error in `ui_event_name`, the new arm fixes it.

- [ ] **Step 3: Commit**

```bash
git add apps/receiver-ui/src-tauri/src/main.rs
git commit -m "feat(receiver-ui): bridge StreamMetricsUpdated to Tauri frontend"
```

---

## Task 3: Extend SSE Parser to Capture Data Lines

**Files:**
- Modify: `services/receiver/src/runtime.rs:544-559` (`consume_sse_line_for_event` function)

Currently `consume_sse_line_for_event` only extracts the event name from `event:` lines and ignores `data:` lines. We need it to also capture the `data:` payload so `metrics_updated` events can be parsed.

- [ ] **Step 1: Write the tests**

Add these tests to the existing `#[cfg(test)]` module in `runtime.rs` (near the existing `sse_event_parsing_*` tests around line 1647):

```rust
#[test]
fn sse_parser_captures_data_payload() {
    let mut pending_event: Option<String> = None;
    let mut pending_data: Option<String> = None;

    assert_eq!(
        consume_sse_line_for_event("event: metrics_updated", &mut pending_event, &mut pending_data),
        None
    );
    assert_eq!(
        consume_sse_line_for_event(
            r#"data: {"stream_id":"abc","raw_count":10}"#,
            &mut pending_event,
            &mut pending_data
        ),
        None
    );
    let result = consume_sse_line_for_event("", &mut pending_event, &mut pending_data);
    assert_eq!(result, Some(("metrics_updated".to_owned(), Some(r#"{"stream_id":"abc","raw_count":10}"#.to_owned()))));
    assert_eq!(pending_event, None);
    assert_eq!(pending_data, None);
}

#[test]
fn sse_parser_returns_none_data_when_no_data_line() {
    let mut pending_event: Option<String> = None;
    let mut pending_data: Option<String> = None;

    consume_sse_line_for_event("event: stream_created", &mut pending_event, &mut pending_data);
    let result = consume_sse_line_for_event("", &mut pending_event, &mut pending_data);
    assert_eq!(result, Some(("stream_created".to_owned(), None)));
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver --lib runtime::tests::sse_parser_captures 2>&1`
Expected: FAIL — function signature doesn't accept `pending_data` parameter.

- [ ] **Step 3: Update `consume_sse_line_for_event` signature and implementation**

Change `consume_sse_line_for_event` in `runtime.rs` (around line 544) from:

```rust
fn consume_sse_line_for_event(line: &str, pending_event: &mut Option<String>) -> Option<String> {
    if line.is_empty() {
        return pending_event.take();
    }
    if line.starts_with(':') {
        return None;
    }
    if let Some(rest) = line.strip_prefix("event:") {
        let event_name = rest.trim();
        if event_name.is_empty() {
            pending_event.take();
        } else {
            *pending_event = Some(event_name.to_owned());
        }
    }
    None
}
```

To:

```rust
fn consume_sse_line_for_event(
    line: &str,
    pending_event: &mut Option<String>,
    pending_data: &mut Option<String>,
) -> Option<(String, Option<String>)> {
    if line.is_empty() {
        let event = pending_event.take();
        let data = pending_data.take();
        return event.map(|e| (e, data));
    }
    if line.starts_with(':') {
        return None;
    }
    if let Some(rest) = line.strip_prefix("event:") {
        let event_name = rest.trim();
        if event_name.is_empty() {
            pending_event.take();
            pending_data.take();
        } else {
            *pending_event = Some(event_name.to_owned());
        }
    } else if let Some(rest) = line.strip_prefix("data:") {
        *pending_data = Some(rest.trim().to_owned());
    }
    None
}
```

- [ ] **Step 4: Update all call sites of `consume_sse_line_for_event`**

In `consume_upstream_dashboard_events` (around line 669), update:

```rust
// Before (around line 643):
let mut pending_event: Option<String> = None;

// After:
let mut pending_event: Option<String> = None;
let mut pending_data: Option<String> = None;
```

And change the call site (around line 669):

```rust
// Before:
if let Some(event_name) = consume_sse_line_for_event(&line, &mut pending_event) {

// After:
if let Some((event_name, _data)) = consume_sse_line_for_event(&line, &mut pending_event, &mut pending_data) {
```

Note: `_data` is unused for now — we'll use it in Task 5.

- [ ] **Step 5: Update existing SSE parser tests**

The existing tests `sse_event_parsing_emits_completed_event_name_on_frame_boundary` and `sse_event_parsing_ignores_comments_and_data_only_frames` (around line 1647) need to be updated to pass the new `pending_data` parameter and handle the new return type `Option<(String, Option<String>)>`. Update them to add `let mut pending_data: Option<String> = None;` and pass `&mut pending_data` as the third argument. Update assertions from `Some("event_name".to_owned())` to `Some(("event_name".to_owned(), None))` (or `Some((..., Some(...)))` if data was present).

- [ ] **Step 6: Run all tests to verify they pass**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver --lib runtime::tests::sse 2>&1`
Expected: PASS for all SSE parser tests.

- [ ] **Step 7: Commit**

```bash
git add services/receiver/src/runtime.rs
git commit -m "feat(receiver): extend SSE parser to capture data payloads"
```

---

## Task 4: Add Stream ID → Key Mapping Infrastructure

**Files:**
- Modify: `services/receiver/src/runtime.rs` (the `run_upstream_dashboard_sse_refresher` function and `consume_upstream_dashboard_events`)
- Modify: `services/receiver/src/control_api.rs` (expose stream_id mapping from `build_streams_response`)

The server's `metrics_updated` SSE events contain `stream_id` (UUID), but the receiver UI is keyed by `forwarder_id/reader_ip`. We need a lookup map built from the upstream stream list data.

- [ ] **Step 1: Add a `StreamIdMap` type alias and builder in `control_api.rs`**

In `services/receiver/src/control_api.rs`, add after the existing imports (around line 15):

```rust
use std::collections::HashMap;

/// Maps server-side stream_id (UUID string) to (forwarder_id, reader_ip) pairs.
pub type StreamIdMap = HashMap<String, (String, String)>;
```

Add a public function after `fetch_server_streams` (around line 568):

```rust
/// Build a stream_id → (forwarder_id, reader_ip) mapping from the upstream stream list.
pub fn build_stream_id_map(streams: &[UpstreamStreamInfo]) -> StreamIdMap {
    streams
        .iter()
        .map(|s| (s.stream_id.clone(), (s.forwarder_id.clone(), s.reader_ip.clone())))
        .collect()
}
```

- [ ] **Step 2: Write a test for the mapping**

In `services/receiver/tests/control_api.rs`, add:

```rust
#[test]
fn build_stream_id_map_creates_correct_mapping() {
    use receiver::control_api::{build_stream_id_map, UpstreamStreamInfo};

    let streams = vec![
        UpstreamStreamInfo {
            stream_id: "aaaa".to_owned(),
            forwarder_id: "fwd-1".to_owned(),
            reader_ip: "10.0.0.1:10000".to_owned(),
            display_alias: None,
            stream_epoch: 1,
            online: true,
            current_epoch_name: None,
        },
    ];
    let map = build_stream_id_map(&streams);
    assert_eq!(map.get("aaaa"), Some(&("fwd-1".to_owned(), "10.0.0.1:10000".to_owned())));
    assert_eq!(map.get("unknown"), None);
}
```

Note: `UpstreamStreamInfo` is currently `pub` (line 505 of `control_api.rs`), so it's accessible in tests.

- [ ] **Step 3: Run test to verify it fails**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver --test control_api build_stream_id_map 2>&1`
Expected: FAIL — `build_stream_id_map` doesn't exist yet.

- [ ] **Step 4: Implement the function (code from Step 1)**

Add the `StreamIdMap` type alias and `build_stream_id_map` function as described in Step 1.

- [ ] **Step 5: Run test to verify it passes**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver --test control_api build_stream_id_map 2>&1`
Expected: PASS

- [ ] **Step 6: Store the map in `AppState`**

Add to `AppState` struct in `control_api.rs` (around line 38):

```rust
pub stream_id_map: Arc<RwLock<StreamIdMap>>,
```

Initialize it in `AppState::with_integrity` (around line 58):

```rust
stream_id_map: Arc::new(RwLock::new(HashMap::new())),
```

- [ ] **Step 7: Populate the map when stream list is fetched**

In `build_streams_response` (around line 235, after `fetch_server_streams` succeeds), add:

```rust
// Update stream_id → key map for SSE metrics resolution
{
    let new_map = build_stream_id_map(&server_streams);
    *self.stream_id_map.write().await = new_map;
}
```

This goes inside the `if let Some(ref server_streams) = server_streams {` block (around line 238), before the `for si in server_streams` loop.

- [ ] **Step 8: Verify compilation**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver 2>&1`
Expected: All existing tests pass. The `setup()` function in `tests/control_api.rs` calls `AppState::new` which calls `with_integrity` — so the new field must have a default in the constructor.

- [ ] **Step 9: Commit**

```bash
git add services/receiver/src/control_api.rs services/receiver/tests/control_api.rs
git commit -m "feat(receiver): add stream_id to key mapping infrastructure"
```

---

## Task 5: Handle `metrics_updated` SSE Events in Runtime

**Files:**
- Modify: `services/receiver/src/runtime.rs:624-681` (`consume_upstream_dashboard_events`)

- [ ] **Step 1: Write the test**

Add to the `#[cfg(test)]` module in `runtime.rs`:

```rust
#[test]
fn parse_metrics_updated_payload() {
    let json = r#"{
        "type": "metrics_updated",
        "stream_id": "aaaa-bbbb",
        "raw_count": 100,
        "dedup_count": 80,
        "retransmit_count": 20,
        "lag_ms": 1500,
        "epoch_raw_count": 50,
        "epoch_dedup_count": 40,
        "epoch_retransmit_count": 10,
        "epoch_lag_ms": 500,
        "epoch_last_received_at": "2026-03-21T12:00:00Z",
        "unique_chips": 30,
        "last_tag_id": "AABBCCDD",
        "last_reader_timestamp": "2026-03-21T12:00:00Z"
    }"#;
    let parsed: UpstreamMetricsPayload = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.stream_id, "aaaa-bbbb");
    assert_eq!(parsed.raw_count, 100);
    assert_eq!(parsed.lag_ms, Some(1500));
    assert_eq!(parsed.unique_chips, 30);
}

#[test]
fn parse_metrics_updated_payload_with_nulls() {
    let json = r#"{
        "type": "metrics_updated",
        "stream_id": "aaaa-bbbb",
        "raw_count": 0,
        "dedup_count": 0,
        "retransmit_count": 0,
        "lag_ms": null,
        "epoch_raw_count": 0,
        "epoch_dedup_count": 0,
        "epoch_retransmit_count": 0,
        "epoch_lag_ms": null,
        "epoch_last_received_at": null,
        "unique_chips": 0,
        "last_tag_id": null,
        "last_reader_timestamp": null
    }"#;
    let parsed: UpstreamMetricsPayload = serde_json::from_str(json).unwrap();
    assert_eq!(parsed.lag_ms, None);
    assert_eq!(parsed.epoch_lag_ms, None);
    assert_eq!(parsed.epoch_last_received_at, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver --lib runtime::tests::parse_metrics 2>&1`
Expected: FAIL — `UpstreamMetricsPayload` doesn't exist.

- [ ] **Step 3: Add `UpstreamMetricsPayload` struct**

Add in `runtime.rs` near the other SSE-related functions (around line 562):

```rust
#[derive(Debug, Deserialize)]
struct UpstreamMetricsPayload {
    stream_id: String,
    raw_count: i64,
    dedup_count: i64,
    retransmit_count: i64,
    lag_ms: Option<u64>,
    epoch_raw_count: i64,
    epoch_dedup_count: i64,
    epoch_retransmit_count: i64,
    epoch_lag_ms: Option<u64>,
    epoch_last_received_at: Option<String>,
    unique_chips: i64,
    // Fields present in payload but not forwarded to UI:
    #[allow(dead_code)]
    last_tag_id: Option<String>,
    #[allow(dead_code)]
    last_reader_timestamp: Option<String>,
}
```

Add `use serde::Deserialize;` at the top of the file if not already imported.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver --lib runtime::tests::parse_metrics 2>&1`
Expected: PASS

- [ ] **Step 5: Add metrics event handling in `consume_upstream_dashboard_events`**

In `consume_upstream_dashboard_events` (around line 669), change:

```rust
// Before:
if let Some((event_name, _data)) = consume_sse_line_for_event(&line, &mut pending_event, &mut pending_data) {
    if should_refresh_stream_snapshot_for_dashboard_event(&event_name) {
        state.emit_streams_snapshot().await;
    }
    if should_emit_receiver_resync_for_dashboard_event(&event_name) {
        state.emit_resync();
    }
    if should_refresh_chip_lookup_for_dashboard_event(&event_name) {
        refresh_chip_lookup(state).await;
    }
}
```

To:

```rust
if let Some((event_name, data)) = consume_sse_line_for_event(&line, &mut pending_event, &mut pending_data) {
    if should_refresh_stream_snapshot_for_dashboard_event(&event_name) {
        state.emit_streams_snapshot().await;
    }
    if should_emit_receiver_resync_for_dashboard_event(&event_name) {
        state.emit_resync();
    }
    if should_refresh_chip_lookup_for_dashboard_event(&event_name) {
        refresh_chip_lookup(state).await;
    }
    if event_name == "metrics_updated" {
        if let Some(data) = data {
            handle_metrics_updated(state, &data).await;
        }
    }
}
```

- [ ] **Step 6: Add the `handle_metrics_updated` function**

Add in `runtime.rs` after the `UpstreamMetricsPayload` struct:

```rust
async fn handle_metrics_updated(state: &Arc<AppState>, data: &str) {
    let payload: UpstreamMetricsPayload = match serde_json::from_str(data) {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!("failed to parse metrics_updated payload: {e}");
            return;
        }
    };

    let stream_id_map = state.stream_id_map.read().await;
    let Some((forwarder_id, reader_ip)) = stream_id_map.get(&payload.stream_id) else {
        return; // Unknown stream, silently ignore
    };

    let _ = state.ui_tx.send(crate::ui_events::ReceiverUiEvent::StreamMetricsUpdated(
        crate::ui_events::StreamMetricsPayload {
            forwarder_id: forwarder_id.clone(),
            reader_ip: reader_ip.clone(),
            raw_count: payload.raw_count,
            dedup_count: payload.dedup_count,
            retransmit_count: payload.retransmit_count,
            lag: payload.lag_ms,
            epoch_raw_count: payload.epoch_raw_count,
            epoch_dedup_count: payload.epoch_dedup_count,
            epoch_retransmit_count: payload.epoch_retransmit_count,
            unique_chips: payload.unique_chips,
            epoch_last_received_at: payload.epoch_last_received_at,
            epoch_lag: payload.epoch_lag_ms,
        },
    ));
}
```

Note how `lag_ms` → `lag` and `epoch_lag_ms` → `epoch_lag` field name mapping happens here.

- [ ] **Step 7: Verify compilation and run all tests**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver 2>&1`
Expected: All tests pass.

- [ ] **Step 8: Commit**

```bash
git add services/receiver/src/runtime.rs
git commit -m "feat(receiver): handle metrics_updated SSE events and forward to UI"
```

---

## Task 6: Add Initial Metrics Fetch on Connection

**Files:**
- Modify: `services/receiver/src/control_api.rs` (add `fetch_stream_metrics` function)
- Modify: `services/receiver/src/runtime.rs` (call fetch on connection)

- [ ] **Step 1: Add `fetch_stream_metrics` HTTP helper**

In `services/receiver/src/control_api.rs`, add after `fetch_server_streams` (around line 568):

```rust
/// Fetch metrics for a single stream from the server's HTTP API.
pub async fn fetch_stream_metrics(
    client: &reqwest::Client,
    ws_url: &str,
    stream_id: &str,
) -> Result<UpstreamMetricsResponse, String> {
    let base = http_base_url(ws_url).ok_or_else(|| "cannot parse upstream URL".to_owned())?;
    let url = format!("{base}/api/v1/streams/{stream_id}/metrics");
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("metrics request failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("metrics returned {}", response.status()));
    }
    response
        .json::<UpstreamMetricsResponse>()
        .await
        .map_err(|e| format!("failed to parse metrics response: {e}"))
}

/// Typed response from GET /api/v1/streams/{id}/metrics.
/// Field names match the server's JSON keys exactly.
#[derive(Debug, Deserialize)]
pub struct UpstreamMetricsResponse {
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
    // Present in response but not forwarded to UI:
    #[allow(dead_code)]
    pub last_tag_id: Option<String>,
    #[allow(dead_code)]
    pub last_reader_timestamp: Option<String>,
}
```

- [ ] **Step 2: Add initial metrics fetch in SSE refresher**

In `runtime.rs`, add a function that fetches metrics for all known streams:

```rust
async fn fetch_initial_metrics(state: &Arc<AppState>) {
    let profile = {
        let db = state.db.lock().await;
        db.load_profile().ok().flatten()
    };
    let Some(profile) = profile else { return };

    let stream_id_map = state.stream_id_map.read().await;
    if stream_id_map.is_empty() {
        return;
    }

    // Collect entries to avoid holding the read lock during HTTP calls
    let entries: Vec<(String, String, String)> = stream_id_map
        .iter()
        .map(|(sid, (fwd, ip))| (sid.clone(), fwd.clone(), ip.clone()))
        .collect();
    drop(stream_id_map);

    // Fetch metrics with concurrency limit of 4
    let semaphore = Arc::new(tokio::sync::Semaphore::new(4));
    let mut handles = Vec::new();

    for (stream_id, forwarder_id, reader_ip) in entries {
        let sem = Arc::clone(&semaphore);
        let client = state.http_client.clone();
        let ws_url = profile.server_url.clone();
        let ui_tx = state.ui_tx.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.ok()?;
            let resp = crate::control_api::fetch_stream_metrics(&client, &ws_url, &stream_id)
                .await
                .ok()?;

            let payload = crate::ui_events::StreamMetricsPayload {
                forwarder_id,
                reader_ip,
                raw_count: resp.raw_count,
                dedup_count: resp.dedup_count,
                retransmit_count: resp.retransmit_count,
                lag: resp.lag_ms,
                epoch_raw_count: resp.epoch_raw_count,
                epoch_dedup_count: resp.epoch_dedup_count,
                epoch_retransmit_count: resp.epoch_retransmit_count,
                unique_chips: resp.unique_chips,
                epoch_last_received_at: resp.epoch_last_received_at,
                epoch_lag: resp.epoch_lag_ms,
            };

            let _ = ui_tx.send(crate::ui_events::ReceiverUiEvent::StreamMetricsUpdated(payload));
            Some(())
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }
}
```

- [ ] **Step 3: Call `fetch_initial_metrics` in the SSE refresher loop**

In `run_upstream_dashboard_sse_refresher` (around line 606), add a call before entering `consume_upstream_dashboard_events`:

```rust
// Before the consume call:
fetch_initial_metrics(&state).await;

match consume_upstream_dashboard_events(&state, &client, &events_url, &profile.token).await
```

This ensures metrics are fetched whenever the SSE connection is (re)established.

- [ ] **Step 4: Verify compilation and run all tests**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test -p receiver 2>&1`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add services/receiver/src/control_api.rs services/receiver/src/runtime.rs
git commit -m "feat(receiver): fetch initial stream metrics on SSE connection"
```

---

## Task 7: Add Frontend Types and Store

**Files:**
- Modify: `apps/receiver-ui/src/lib/api.ts:36-43`
- Modify: `apps/receiver-ui/src/lib/store.svelte.ts`
- Modify: `apps/receiver-ui/src/lib/sse.ts`

- [ ] **Step 1: Add `StreamMetrics` interface to `api.ts`**

In `apps/receiver-ui/src/lib/api.ts`, add after the `LastRead` interface (around line 43):

```typescript
export interface StreamMetrics {
  forwarder_id: string;
  reader_ip: string;
  raw_count: number;
  dedup_count: number;
  retransmit_count: number;
  lag: number | null;
  epoch_raw_count: number;
  epoch_dedup_count: number;
  epoch_retransmit_count: number;
  unique_chips: number;
  epoch_last_received_at: string | null;
  epoch_lag: number | null;
}
```

- [ ] **Step 2: Add `streamMetrics` map to store**

In `apps/receiver-ui/src/lib/store.svelte.ts`, add to the `store` `$state` object (around line 53, after `lastReads`):

```typescript
streamMetrics: new Map<string, api.StreamMetrics>(),
```

- [ ] **Step 3: Add SSE listener for `stream_metrics_updated`**

In `apps/receiver-ui/src/lib/sse.ts`:

Add a new payload type (around line 33, after `LastReadPayload`):

```typescript
type StreamMetricsPayload = {
  forwarder_id: string;
  reader_ip: string;
  raw_count: number;
  dedup_count: number;
  retransmit_count: number;
  lag: number | null;
  epoch_raw_count: number;
  epoch_dedup_count: number;
  epoch_retransmit_count: number;
  unique_chips: number;
  epoch_last_received_at: string | null;
  epoch_lag: number | null;
};
```

Add a new callback to the `SseCallbacks` type (around line 44, after `onLastRead`):

```typescript
onStreamMetricsUpdated: (metrics: import("./api").StreamMetrics) => void;
```

Add a new `listen` call inside the `Promise.all` in `initSSE` (after the `last_read` listener, around line 92):

```typescript
listen<StreamMetricsPayload>("stream_metrics_updated", (event) => {
  callbacks.onStreamMetricsUpdated(event.payload);
}),
```

- [ ] **Step 4: Wire the callback in `initStore`**

In `apps/receiver-ui/src/lib/store.svelte.ts`, in the `initSSE` call (around line 891, after `onLastRead`), add:

```typescript
onStreamMetricsUpdated: (metrics) => {
  const key = streamKey(metrics.forwarder_id, metrics.reader_ip);
  const next = new Map(store.streamMetrics);
  next.set(key, metrics);
  store.streamMetrics = next;
},
```

Also add cleanup logic to the existing callbacks:

In the `onResync` callback (around line 880), add after the existing `void loadAll()`:

```typescript
store.streamMetrics = new Map();
```

In the `onStatusChanged` callback, add a check to clear metrics on disconnect:

```typescript
onStatusChanged: (s) => {
  store.status = s;
  if (s.connection_state === "disconnected") {
    store.streamMetrics = new Map();
  }
},
```

In the `onStreamsSnapshot` callback, prune stale metrics keys:

```typescript
// After setting store.streams, prune stale metrics
const currentKeys = new Set(resp.streams.map(s => streamKey(s.forwarder_id, s.reader_ip)));
const prunedMetrics = new Map(store.streamMetrics);
for (const key of prunedMetrics.keys()) {
  if (!currentKeys.has(key)) prunedMetrics.delete(key);
}
store.streamMetrics = prunedMetrics;
```

- [ ] **Step 5: Verify frontend compilation**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2/apps/receiver-ui && npm run check 2>&1`
Expected: No type errors.

- [ ] **Step 6: Commit**

```bash
git add apps/receiver-ui/src/lib/api.ts apps/receiver-ui/src/lib/store.svelte.ts apps/receiver-ui/src/lib/sse.ts
git commit -m "feat(receiver-ui): add StreamMetrics type, store, and SSE listener"
```

---

## Task 8: Render Metrics in Expanded Stream Row

**Files:**
- Modify: `apps/receiver-ui/src/lib/components/StreamsTab.svelte:197-231`

- [ ] **Step 1: Add formatting helper functions**

In `StreamsTab.svelte`, add these functions in the `<script>` block (after the existing `formatLastRead` function, around line 108):

```typescript
function formatLag(lag: number | null): string {
  if (lag === null) return "N/A (no events yet)";
  if (lag < 1000) return `${lag} ms`;
  return `${(lag / 1000).toFixed(1)} s`;
}

function formatDuration(ms: number): string {
  if (ms < 1000) return "< 1s";
  const totalSeconds = Math.floor(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours}h ${minutes}m ${seconds}s`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}
```

- [ ] **Step 2: Add reactive `timeSinceLastRead` state**

In the `<script>` block, add:

```typescript
let timeSinceLastRead = $state<Record<string, string>>({});

$effect(() => {
  if (!expandedKey) return;

  const key = expandedKey;
  const metrics = store.streamMetrics.get(key);

  if (!metrics?.epoch_last_received_at) {
    timeSinceLastRead = { ...timeSinceLastRead, [key]: "N/A (no events in epoch)" };
    return;
  }

  const update = () => {
    const now = Date.now();
    const lastAt = new Date(metrics.epoch_last_received_at!).getTime();
    timeSinceLastRead = { ...timeSinceLastRead, [key]: formatDuration(now - lastAt) };
  };

  update();
  const interval = setInterval(update, 1000);
  return () => clearInterval(interval);
});
```

- [ ] **Step 3: Replace the "Reads" line in the expanded row with metrics sections**

In the expanded row template (around line 197-231), find the existing reads display (around line 223):

```svelte
{#if stream.subscribed && stream.reads_total !== undefined}
  <div>
    <dt class="text-muted text-xs">Reads</dt>
    <dd>{stream.reads_total.toLocaleString()} total, {stream.reads_epoch?.toLocaleString() ?? 0} epoch</dd>
  </div>
{/if}
```

Replace it with the metrics sections. After the existing Epoch `<div>` (around line 222) and before the controls row, add:

```svelte
{@const metrics = store.streamMetrics.get(streamKey(stream.forwarder_id, stream.reader_ip))}
{#if metrics}
  <!-- Lifetime Metrics -->
  <div class="mt-2">
    <p class="text-muted text-xs font-medium mb-1">Lifetime</p>
    <div class="grid grid-cols-2 gap-x-4 gap-y-1">
      <div>
        <dt class="text-muted text-xs" title="Total frames received including retransmits">Raw count</dt>
        <dd>{metrics.raw_count.toLocaleString()}</dd>
      </div>
      <div>
        <dt class="text-muted text-xs" title="Unique frames after deduplication">Dedup count</dt>
        <dd>{metrics.dedup_count.toLocaleString()}</dd>
      </div>
      <div>
        <dt class="text-muted text-xs" title="Duplicate frames that matched existing events">Retransmit</dt>
        <dd>{metrics.retransmit_count.toLocaleString()}</dd>
      </div>
      <div>
        <dt class="text-muted text-xs" title="Time since the last unique frame was received">Lag</dt>
        <dd>{formatLag(metrics.lag)}</dd>
      </div>
    </div>
  </div>

  <!-- Current Epoch Metrics -->
  <div class="mt-2">
    <p class="text-muted text-xs font-medium mb-1">Current Epoch</p>
    <div class="grid grid-cols-2 gap-x-4 gap-y-1">
      <div>
        <dt class="text-muted text-xs" title="Frames received in the current epoch">Raw (epoch)</dt>
        <dd>{metrics.epoch_raw_count.toLocaleString()}</dd>
      </div>
      <div>
        <dt class="text-muted text-xs" title="Unique frames in the current epoch">Dedup (epoch)</dt>
        <dd>{metrics.epoch_dedup_count.toLocaleString()}</dd>
      </div>
      <div>
        <dt class="text-muted text-xs" title="Duplicate frames in the current epoch">Retransmit (epoch)</dt>
        <dd>{metrics.epoch_retransmit_count.toLocaleString()}</dd>
      </div>
      <div>
        <dt class="text-muted text-xs" title="Distinct chip IDs detected in the current epoch">Unique chips</dt>
        <dd>{metrics.unique_chips.toLocaleString()}</dd>
      </div>
      <div>
        <dt class="text-muted text-xs" title="Timestamp of the last unique frame in the current epoch">Last read</dt>
        <dd>{metrics.epoch_last_received_at ? new Date(metrics.epoch_last_received_at).toLocaleString() : "N/A (no events in epoch)"}</dd>
      </div>
      <div>
        <dt class="text-muted text-xs" title="Live-updating elapsed time since last unique frame">Time since last read</dt>
        <dd>{timeSinceLastRead[streamKey(stream.forwarder_id, stream.reader_ip)] ?? "—"}</dd>
      </div>
    </div>
  </div>
{:else}
  <p class="text-muted text-xs mt-2">Metrics unavailable</p>
{/if}
```

Remove the old "Reads" `{#if}` block that was replaced.

- [ ] **Step 4: Verify frontend compilation**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2/apps/receiver-ui && npm run check 2>&1`
Expected: No type errors.

- [ ] **Step 5: Commit**

```bash
git add apps/receiver-ui/src/lib/components/StreamsTab.svelte
git commit -m "feat(receiver-ui): render stream metrics in expanded row"
```

---

## Task 9: Update Frontend Tests

**Files:**
- Modify: `apps/receiver-ui/src/lib/components/StreamsTab.test.ts`

- [ ] **Step 1: Add metrics rendering test**

In `StreamsTab.test.ts`, add `fireEvent` to the existing `@testing-library/svelte` import (change `{ render, screen }` to `{ fireEvent, render, screen }`). Then add to the existing `describe("StreamsTab")` block:

```typescript
import { streamKey } from "$lib/store.svelte";
// (streamKey is already imported at line 5)

it("shows metrics in expanded row when available", async () => {
  const key = streamKey("fwd-1", "10.0.0.1:10000");
  store.streamMetrics = new Map([
    [
      key,
      {
        forwarder_id: "fwd-1",
        reader_ip: "10.0.0.1:10000",
        raw_count: 1500,
        dedup_count: 1200,
        retransmit_count: 300,
        lag: 2500,
        epoch_raw_count: 500,
        epoch_dedup_count: 400,
        epoch_retransmit_count: 100,
        unique_chips: 75,
        epoch_last_received_at: "2026-03-21T12:00:00Z",
        epoch_lag: 1000,
      },
    ],
  ]);

  render(StreamsTab);

  // Click to expand the row
  const row = screen.getByText("Finish").closest("tr")!;
  await fireEvent.click(row);

  // Verify lifetime metrics
  expect(screen.getByText("1,500")).toBeInTheDocument();    // raw count
  expect(screen.getByText("1,200")).toBeInTheDocument();    // dedup count
  expect(screen.getByText("300")).toBeInTheDocument();       // retransmit
  expect(screen.getByText("2.5 s")).toBeInTheDocument();     // lag

  // Verify epoch metrics
  expect(screen.getByText("75")).toBeInTheDocument();        // unique chips

  // Verify help text (title attributes)
  expect(screen.getByTitle("Total frames received including retransmits")).toBeInTheDocument();
  expect(screen.getByTitle("Distinct chip IDs detected in the current epoch")).toBeInTheDocument();
});

it("shows 'Metrics unavailable' when no metrics data", async () => {
  store.streamMetrics = new Map();

  render(StreamsTab);

  const row = screen.getByText("Finish").closest("tr")!;
  await fireEvent.click(row);

  expect(screen.getByText("Metrics unavailable")).toBeInTheDocument();
});
```

- [ ] **Step 2: Run tests**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2/apps/receiver-ui && npx vitest run src/lib/components/StreamsTab.test.ts 2>&1`
Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add apps/receiver-ui/src/lib/components/StreamsTab.test.ts
git commit -m "test(receiver-ui): add stream metrics rendering tests"
```

---

## Task 10: Final Verification

- [ ] **Step 1: Run full Rust test suite**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo test 2>&1`
Expected: All tests pass.

- [ ] **Step 2: Run full frontend test suite**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2/apps/receiver-ui && npx vitest run 2>&1`
Expected: All tests pass.

- [ ] **Step 3: Run clippy**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2 && cargo clippy --all-targets 2>&1`
Expected: No warnings.

- [ ] **Step 4: Run frontend type check**

Run: `cd /Users/iwismer/Development/conductor/workspaces/rusty-timer/kingston-v2/apps/receiver-ui && npm run check 2>&1`
Expected: No type errors.
