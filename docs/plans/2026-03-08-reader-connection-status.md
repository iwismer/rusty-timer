# Reader Connection Status Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Surface reader↔forwarder TCP connection status on the server, dashboard, and receiver — independent of the existing forwarder↔server "online" status.

**Architecture:** The forwarder sends a new `ReaderStatusUpdate` WsMessage on each reader TCP state transition and as a burst after `ForwarderHello`. The server persists `reader_connected` in the `streams` table, propagates via SSE to the dashboard and via a new `ReaderStatusChanged` WsMessage to receivers.

**Tech Stack:** Rust, rt-protocol (serde), sqlx/Postgres, Axum SSE, Tauri/SvelteKit (receiver UI)

---

### Task 1: Add protocol message types (`rt-protocol`)

**Files:**
- Modify: `crates/rt-protocol/src/lib.rs`

**Step 1: Add `ReaderStatusUpdate` struct**

After the `ForwarderHello` struct (around line 73), add:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderStatusUpdate {
    pub reader_ip: String,
    pub connected: bool,
}
```

**Step 2: Add `ReaderStatusChanged` struct**

Directly after `ReaderStatusUpdate`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReaderStatusChanged {
    pub stream_id: Uuid,
    pub reader_ip: String,
    pub connected: bool,
}
```

This requires `use uuid::Uuid;` — check if already imported. If not, add it. The `uuid` crate with `serde` feature should already be a dependency of `rt-protocol` (used by other structs like `Heartbeat`).

**Step 3: Add variants to `WsMessage` enum**

In the `WsMessage` enum (line ~316–333), add two new variants:

```rust
    ReaderStatusUpdate(ReaderStatusUpdate),
    ReaderStatusChanged(ReaderStatusChanged),
```

Place them after `RestartResponse(RestartResponse)`.

**Step 4: Write a round-trip serde test**

Add a `#[cfg(test)]` module at the bottom of `lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_status_update_round_trip() {
        let msg = WsMessage::ReaderStatusUpdate(ReaderStatusUpdate {
            reader_ip: "192.168.1.10".to_string(),
            connected: true,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"reader_status_update\""));
        assert!(json.contains("\"connected\":true"));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }

    #[test]
    fn reader_status_changed_round_trip() {
        let msg = WsMessage::ReaderStatusChanged(ReaderStatusChanged {
            stream_id: Uuid::nil(),
            reader_ip: "192.168.1.10".to_string(),
            connected: false,
        });
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"kind\":\"reader_status_changed\""));
        assert!(json.contains("\"connected\":false"));
        let parsed: WsMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, msg);
    }
}
```

**Step 5: Run tests**

Run: `cargo test -p rt-protocol`
Expected: Both new tests pass, existing code compiles.

**Step 6: Commit**

```bash
git add crates/rt-protocol/src/lib.rs
git commit -m "feat(protocol): add ReaderStatusUpdate and ReaderStatusChanged message types"
```

---

### Task 2: Server migration — add `reader_connected` column

**Files:**
- Create: `services/server/migrations/0010_reader_connected.sql`

**Step 1: Write migration**

```sql
ALTER TABLE streams ADD COLUMN reader_connected BOOLEAN NOT NULL DEFAULT false;
```

**Step 2: Run migration and regenerate sqlx offline cache**

Run: `cd services/server && cargo sqlx prepare`
Expected: `.sqlx/` files updated with the new column.

**Step 3: Commit**

```bash
git add services/server/migrations/0010_reader_connected.sql services/server/.sqlx/
git commit -m "feat(server): add reader_connected column to streams table"
```

---

### Task 3: Server repo — add `set_reader_connected` function

**Files:**
- Modify: `services/server/src/repo/events.rs`

**Step 1: Add `set_reader_connected` function**

After `set_stream_online` (around line 200), add:

```rust
pub async fn set_reader_connected(
    pool: &PgPool,
    stream_id: Uuid,
    connected: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE streams SET reader_connected = $1 WHERE stream_id = $2",
        connected,
        stream_id
    )
    .execute(pool)
    .await?;
    Ok(())
}
```

**Step 2: Update `set_stream_online` to also clear `reader_connected` when going offline**

Modify the existing `set_stream_online` function. When `online = false`, also set `reader_connected = false`:

```rust
pub async fn set_stream_online(
    pool: &PgPool,
    stream_id: Uuid,
    online: bool,
) -> Result<(), sqlx::Error> {
    if online {
        sqlx::query!(
            "UPDATE streams SET online = $1 WHERE stream_id = $2",
            online,
            stream_id
        )
        .execute(pool)
        .await?;
    } else {
        sqlx::query!(
            "UPDATE streams SET online = false, reader_connected = false WHERE stream_id = $1",
            stream_id
        )
        .execute(pool)
        .await?;
    }
    Ok(())
}
```

**Step 3: Regenerate sqlx offline cache**

Run: `cd services/server && cargo sqlx prepare`

**Step 4: Run server tests**

Run: `cargo test -p rusty-timer-server`
Expected: All existing tests pass.

**Step 5: Commit**

```bash
git add services/server/src/repo/events.rs services/server/.sqlx/
git commit -m "feat(server): add set_reader_connected repo function"
```

---

### Task 4: Server dashboard events — add `reader_connected` to SSE

**Files:**
- Modify: `services/server/src/dashboard_events.rs`

**Step 1: Add `reader_connected` to `StreamCreated`**

In the `StreamCreated` variant (around line 28), add after the `online` field:

```rust
        reader_connected: bool,
```

**Step 2: Add `reader_connected` to `StreamUpdated`**

In the `StreamUpdated` variant (around line 40), add after the `online` field:

```rust
        #[serde(skip_serializing_if = "Option::is_none")]
        reader_connected: Option<bool>,
```

**Step 3: Fix all compilation errors**

Every place that constructs `StreamCreated` or `StreamUpdated` must now include `reader_connected`. Search for these construction sites in `ws_forwarder.rs` and other files. Add `reader_connected: false` to `StreamCreated` (new streams start with reader disconnected) and `reader_connected: None` to `StreamUpdated` where it's not a reader-status change.

**Step 4: Run server tests**

Run: `cargo test -p rusty-timer-server`
Expected: All tests pass.

**Step 5: Commit**

```bash
git add services/server/src/dashboard_events.rs services/server/src/ws_forwarder.rs
git commit -m "feat(server): add reader_connected to dashboard SSE events"
```

---

### Task 5: Server WS handler — handle `ReaderStatusUpdate`

**Files:**
- Modify: `services/server/src/ws_forwarder.rs`
- Modify: `services/server/src/repo/events.rs` (if needed for sqlx cache)

**Step 1: Add match arm for `ReaderStatusUpdate` in the main message dispatch**

In `ws_forwarder.rs`, in the message match block (around line 334), add a new arm:

```rust
WsMessage::ReaderStatusUpdate(update) => {
    // Find the stream_id for this reader_ip
    if let Some(sid) = stream_map.get(&update.reader_ip) {
        let _ = set_reader_connected(&state.pool, *sid, update.connected).await;
        let _ = state.dashboard_tx.send(DashboardEvent::StreamUpdated {
            stream_id: *sid,
            online: None,
            stream_epoch: None,
            display_alias: None,
            forwarder_display_name: None,
            reader_connected: Some(update.connected),
        });
        // Forward to receivers via the stream's broadcast channel
        if let Some(tx) = state.get_broadcast(*sid) {
            let changed = WsMessage::ReaderStatusChanged(rt_protocol::ReaderStatusChanged {
                stream_id: *sid,
                reader_ip: update.reader_ip.clone(),
                connected: update.connected,
            });
            let _ = tx.send(changed);
        }
    }
}
```

Note: Check how the server forwards messages to receivers. The server has per-stream broadcast channels (`state.get_or_create_broadcast(sid)`). Look at how `ReceiverEventBatch` is sent to receivers — follow the same pattern for `ReaderStatusChanged`. The broadcast channel type may be `broadcast::Sender<WsMessage>` or a different envelope type. Match the existing pattern exactly.

**Step 2: Regenerate sqlx offline cache if needed**

Run: `cd services/server && cargo sqlx prepare`

**Step 3: Run server tests**

Run: `cargo test -p rusty-timer-server`
Expected: All tests pass.

**Step 4: Commit**

```bash
git add services/server/src/ws_forwarder.rs services/server/.sqlx/
git commit -m "feat(server): handle ReaderStatusUpdate from forwarder"
```

---

### Task 6: Server HTTP API — add `reader_connected` to streams list

**Files:**
- Modify: `services/server/src/http/streams.rs`

**Step 1: Add `reader_connected` to the SQL query and JSON response**

In `list_streams` (line ~14), update the SQL SELECT to include `s.reader_connected`. Then in the JSON mapping (line ~46), add `"reader_connected": row.reader_connected`.

**Step 2: Regenerate sqlx offline cache**

Run: `cd services/server && cargo sqlx prepare`

**Step 3: Run server tests**

Run: `cargo test -p rusty-timer-server`
Expected: All tests pass.

**Step 4: Commit**

```bash
git add services/server/src/http/streams.rs services/server/.sqlx/
git commit -m "feat(server): include reader_connected in streams API response"
```

---

### Task 7: Forwarder — send `ReaderStatusUpdate` on state changes

**Files:**
- Modify: `services/forwarder/src/main.rs`
- Modify: `services/forwarder/src/uplink.rs`

This task has two parts: (A) sending status updates on reader state transitions, and (B) sending initial burst after `ForwarderHello`.

**Step 1: Add a channel for reader status updates to reach the uplink task**

The forwarder runs `run_reader` and `run_uplink` as separate tasks. They need to communicate. Add an `mpsc` channel that `run_reader` sends `ReaderStatusUpdate` messages on, and `run_uplink` receives from.

In `main.rs`, where the tasks are spawned, create:
```rust
let (reader_status_tx, reader_status_rx) = tokio::sync::mpsc::unbounded_channel::<rt_protocol::ReaderStatusUpdate>();
```

Pass `reader_status_tx.clone()` to each `run_reader` call, and pass `reader_status_rx` to `run_uplink`.

**Step 2: Send `ReaderStatusUpdate` from `run_reader` on state transitions**

In `run_reader`, after each call to `status.update_reader_state(...)` that transitions to `Connected` or `Disconnected`, send on the channel:

```rust
let _ = reader_status_tx.send(rt_protocol::ReaderStatusUpdate {
    reader_ip: reader_ip.clone(),
    connected: true, // or false for Disconnected
});
```

Do NOT send for `Connecting` — only `Connected` and `Disconnected`.

**Step 3: Drain and send status updates in `run_uplink`**

In `run_uplink`, add `reader_status_rx` as a parameter. In the main `tokio::select!` loop (line ~957), add a new arm:

```rust
Some(update) = reader_status_rx.recv() => {
    if let Err(_) = session.send_message(&WsMessage::ReaderStatusUpdate(update)).await {
        break 'uplink;
    }
}
```

**Step 4: Send initial burst after `ForwarderHello`**

After `connect_with_readers` succeeds (around line 786), read current reader states from `status`/`subsystem` and send a `ReaderStatusUpdate` for each reader:

```rust
{
    let ss = subsystem.lock().await;
    for (ip, reader) in &ss.readers {
        let connected = reader.state == ReaderConnectionState::Connected;
        let update = WsMessage::ReaderStatusUpdate(rt_protocol::ReaderStatusUpdate {
            reader_ip: ip.clone(),
            connected,
        });
        if let Err(_) = session.send_message(&update).await {
            continue; // will reconnect
        }
    }
}
```

**Step 5: Run forwarder tests**

Run: `cargo test -p rusty-timer-forwarder`
Expected: All existing tests pass. Some tests may need updating if they use `run_reader` or `run_uplink` signatures that changed.

**Step 6: Commit**

```bash
git add services/forwarder/src/main.rs services/forwarder/src/uplink.rs
git commit -m "feat(forwarder): send ReaderStatusUpdate on reader state changes"
```

---

### Task 8: Receiver — handle `ReaderStatusChanged`

**Files:**
- Modify: `services/receiver/src/session.rs`
- Potentially modify receiver state/UI event types

**Step 1: Understand receiver architecture**

The receiver's `run_session_loop` in `session.rs:99-160` dispatches on `WsMessage` variants. Unknown variants are logged with `debug!(?o, "ignoring")` at line 150. The receiver has `SessionLoopDeps` with channels for events and UI.

**Step 2: Add a match arm for `ReaderStatusChanged`**

In the message dispatch (around line 148, before the catch-all), add:

```rust
WsMessage::ReaderStatusChanged(status) => {
    tracing::info!(
        stream_id = %status.stream_id,
        reader_ip = %status.reader_ip,
        connected = status.connected,
        "reader connection status changed"
    );
    if !status.connected {
        // Surface warning via UI log
        let warning = format!(
            "Reader {} disconnected from forwarder (stream {})",
            status.reader_ip,
            status.stream_id
        );
        deps.log_warning(&warning);
    }
}
```

Check how `deps` exposes logging/UI events. The `ReceiverModeApplied` handler at lines 133-146 broadcasts warnings as `LogEntry` UI events — follow the same pattern. The exact method may be `deps.ui_tx.send(...)` or similar.

**Step 3: Run receiver tests**

Run: `cargo test -p rusty-timer-receiver`
Expected: All tests pass.

**Step 4: Commit**

```bash
git add services/receiver/src/session.rs
git commit -m "feat(receiver): handle ReaderStatusChanged with warning on disconnect"
```

---

### Task 9: Integration test — reader status propagation

**Files:**
- Modify or create test in: `tests/integration/`

**Step 1: Write an integration test**

Using the existing `MockWsClient` pattern from `e2e_forwarder_server_receiver.rs`, write a test that:

1. Connects a mock forwarder via WS, sends `ForwarderHello` with `reader_ips: ["192.168.1.10"]`
2. Sends `ReaderStatusUpdate { reader_ip: "192.168.1.10", connected: true }`
3. Verifies `GET /api/v1/streams` returns `reader_connected: true` for that stream
4. Sends `ReaderStatusUpdate { reader_ip: "192.168.1.10", connected: false }`
5. Verifies `GET /api/v1/streams` returns `reader_connected: false`
6. Verifies that on forwarder WS disconnect, both `online` and `reader_connected` are `false`

Follow the existing test patterns — use `testcontainers` for Postgres, spawn the server, use `MockWsClient` for the forwarder connection.

**Step 2: Run integration tests**

Run: `cargo test -p integration-tests --test-threads=4` (or the specific test file)
Expected: New test passes.

**Step 3: Commit**

```bash
git add tests/integration/
git commit -m "test: add integration test for reader connection status propagation"
```

---

### Task 10: Dashboard — display `reader_connected` status

**Files:**
- Modify: `apps/dashboard/src/` (SvelteKit components)

**Step 1: Identify the stream list/status component**

Find the component that displays the stream list and its online/offline status indicators. Look in `apps/dashboard/src/routes/` or `apps/dashboard/src/lib/components/`.

**Step 2: Add `reader_connected` to the stream data model**

Update the TypeScript type/interface for streams to include `reader_connected: boolean`.

**Step 3: Update SSE event handling**

Where `stream_created` and `stream_updated` SSE events are processed, extract the new `reader_connected` field.

**Step 4: Update the status display**

Show both statuses. When `online=true` but `reader_connected=false`, display a warning indicator (e.g., yellow/amber) with text like "Reader disconnected". When both are true, show the normal "Online" indicator.

**Step 5: Run dashboard tests**

Run: `cd apps/dashboard && npm test`
Expected: Tests pass (may need to update test fixtures with new field).

**Step 6: Commit**

```bash
git add apps/dashboard/
git commit -m "feat(dashboard): display reader connection status on stream list"
```
