# Race/Epoch Receiver Selection v1.1 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a v1.1 receiver protocol and server/receiver/UI behavior that supports manual mode or race-based epoch selection (`all` or `current`) with deterministic backfill from selected epoch starts.

**Architecture:** Keep `/ws/v1/receivers` unchanged and add `/ws/v1.1/receivers` with new selection messages. Server resolves race selection into `(stream, epoch)` targets, drives replay/filtering, and persists per-epoch receiver cursors. Add explicit `stream_epoch_races` mappings plus activation APIs for pre-creating and activating next epoch online.

**Tech Stack:** Rust (axum/sqlx/tokio), PostgreSQL migrations, SQLite receiver cache, SvelteKit (`apps/server-ui`, `apps/receiver-ui`), WebSocket protocol (`crates/rt-protocol`).

---

### Task 1: Add v1.1 protocol types in `rt-protocol`

**Files:**
- Modify: `crates/rt-protocol/src/lib.rs`
- Modify: `crates/rt-protocol/tests/contract_examples.rs`
- Modify: `crates/rt-protocol/README.md`

**Step 1: Write the failing test**

```rust
#[test]
fn receiver_hello_v11_with_race_selection_round_trips() {
    let msg = WsMessage::ReceiverHelloV11(ReceiverHelloV11 {
        receiver_id: "rcv-1".to_owned(),
        selection: ReceiverSelection::Race {
            race_id: "11111111-1111-1111-1111-111111111111".to_owned(),
            epoch_scope: EpochScope::Current,
        },
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, msg);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p rt-protocol contract_examples -- --nocapture`
Expected: FAIL because `ReceiverHelloV11` / `ReceiverSelection` kinds do not exist.

**Step 3: Write minimal implementation**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ReceiverSelection {
    Manual { streams: Vec<StreamRef> },
    Race { race_id: String, epoch_scope: EpochScope },
}
```

Add v1.1 message variants:
- `ReceiverHelloV11`
- `ReceiverSetSelection`
- `ReceiverSelectionApplied`

**Step 4: Run test to verify it passes**

Run: `cargo test -p rt-protocol contract_examples -- --nocapture`
Expected: PASS for v1.1 round-trip tests.

**Step 5: Commit**

```bash
git add crates/rt-protocol/src/lib.rs crates/rt-protocol/tests/contract_examples.rs crates/rt-protocol/README.md
git commit -m "feat(protocol): add receiver v1.1 selection message types"
```

### Task 2: Add DB migration for stream-epoch race mappings and per-epoch cursors

**Files:**
- Create: `services/server/migrations/0007_stream_epoch_races_and_receiver_epoch_cursors.sql`
- Modify: `services/server/tests/migration_smoke.rs`

**Step 1: Write the failing test**

```rust
#[test]
fn migration_creates_stream_epoch_races_table() {
    let sql = load_migration("0007_stream_epoch_races_and_receiver_epoch_cursors.sql");
    assert!(sql.contains("CREATE TABLE stream_epoch_races"));
    assert!(sql.contains("PRIMARY KEY (stream_id, stream_epoch)"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p server --test migration_smoke migration_creates_stream_epoch_races_table`
Expected: FAIL because migration file/table is missing.

**Step 3: Write minimal implementation**

```sql
CREATE TABLE stream_epoch_races (
  stream_id UUID NOT NULL REFERENCES streams(stream_id) ON DELETE CASCADE,
  stream_epoch BIGINT NOT NULL,
  race_id UUID NOT NULL REFERENCES races(race_id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (stream_id, stream_epoch)
);
CREATE INDEX idx_stream_epoch_races_race_id ON stream_epoch_races(race_id);
```

Also migrate `receiver_cursors` to PK `(receiver_id, stream_id, stream_epoch)`.

**Step 4: Run test to verify it passes**

Run: `cargo test -p server --test migration_smoke`
Expected: PASS with new migration assertions.

**Step 5: Commit**

```bash
git add services/server/migrations/0007_stream_epoch_races_and_receiver_epoch_cursors.sql services/server/tests/migration_smoke.rs
git commit -m "feat(server): add stream-epoch race mapping migration"
```

### Task 3: Implement server repo layer for epoch mappings and per-epoch cursors

**Files:**
- Create: `services/server/src/repo/stream_epoch_races.rs`
- Modify: `services/server/src/repo/mod.rs`
- Modify: `services/server/src/repo/receiver_cursors.rs`
- Create: `services/server/tests/receiver_epoch_cursors.rs`

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn cursor_is_persisted_per_epoch() {
    // save epoch=1 and epoch=2 independently, ensure both rows exist
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p server --test receiver_epoch_cursors cursor_is_persisted_per_epoch`
Expected: FAIL because current upsert key overwrites across epochs.

**Step 3: Write minimal implementation**

```rust
pub async fn upsert_cursor(
    pool: &PgPool,
    receiver_id: &str,
    stream_id: Uuid,
    stream_epoch: i64,
    last_seq: i64,
) -> Result<(), sqlx::Error> { /* upsert by receiver+stream+epoch */ }
```

Add mapping repo operations:
- set/unset `(stream_id, epoch) -> race`
- list mappings by race
- list mapped epochs by stream (including empty)

**Step 4: Run test to verify it passes**

Run: `cargo test -p server --test receiver_epoch_cursors`
Expected: PASS for per-epoch cursor behavior.

**Step 5: Commit**

```bash
git add services/server/src/repo/stream_epoch_races.rs services/server/src/repo/mod.rs services/server/src/repo/receiver_cursors.rs services/server/tests/receiver_epoch_cursors.rs
git commit -m "feat(server): add epoch-race repo and per-epoch receiver cursors"
```

### Task 4: Add server HTTP APIs for epoch-race mapping and activate-next

**Files:**
- Create: `services/server/src/http/stream_epoch_races.rs`
- Modify: `services/server/src/http/mod.rs`
- Modify: `services/server/src/lib.rs`
- Modify: `services/server/src/http/streams.rs`
- Modify: `services/server/src/state.rs`
- Create: `services/server/tests/http_stream_epoch_races.rs`

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn activate_next_returns_409_when_forwarder_offline() {
    // POST /api/v1/races/:race_id/streams/:stream_id/epochs/activate-next
    // expect 409
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p server --test http_stream_epoch_races activate_next_returns_409_when_forwarder_offline`
Expected: FAIL because route/handler does not exist.

**Step 3: Write minimal implementation**

```rust
// PUT /api/v1/streams/:stream_id/epochs/:epoch/race
// GET /api/v1/races/:race_id/stream-epochs
// POST /api/v1/races/:race_id/streams/:stream_id/epochs/activate-next
```

For `activate-next`:
- verify forwarder sender exists (online)
- compute `next = current + 1`
- write mapping for `(stream, next)`
- send `EpochResetCommand`
- advance `streams.stream_epoch` immediately

**Step 4: Run test to verify it passes**

Run: `cargo test -p server --test http_stream_epoch_races`
Expected: PASS for mapping CRUD and activate-next edge cases.

**Step 5: Commit**

```bash
git add services/server/src/http/stream_epoch_races.rs services/server/src/http/mod.rs services/server/src/lib.rs services/server/src/http/streams.rs services/server/src/state.rs services/server/tests/http_stream_epoch_races.rs
git commit -m "feat(server): add epoch-race APIs and activate-next endpoint"
```

### Task 5: Extend stream epoch listing to include mapped empty epochs

**Files:**
- Modify: `services/server/src/http/streams.rs`
- Modify: `services/server/tests/http_streams.rs`

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn list_epochs_includes_mapped_epoch_without_events() {
    // map epoch N in stream_epoch_races with no events
    // GET /api/v1/streams/:stream_id/epochs includes epoch N
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p server --test http_streams list_epochs_includes_mapped_epoch_without_events`
Expected: FAIL because current query only scans `events`.

**Step 3: Write minimal implementation**

```sql
-- union mapped epochs with event-backed epochs
-- left join events aggregate for nullable counts/timestamps
```

Return `event_count: 0`, `first_event_at: null`, `last_event_at: null` when empty.

**Step 4: Run test to verify it passes**

Run: `cargo test -p server --test http_streams list_epochs_includes_mapped_epoch_without_events`
Expected: PASS.

**Step 5: Commit**

```bash
git add services/server/src/http/streams.rs services/server/tests/http_streams.rs
git commit -m "feat(server): include mapped empty epochs in stream epoch listing"
```

### Task 6: Implement server `/ws/v1.1/receivers` handshake + selection updates

**Files:**
- Modify: `services/server/src/lib.rs`
- Modify: `services/server/src/ws_receiver.rs`
- Create: `services/server/tests/receiver_selection_v11.rs`

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn receiver_v11_requires_selection_in_hello() {
    // connect to /ws/v1.1/receivers
    // send invalid hello without selection
    // expect protocol error
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p server --test receiver_selection_v11 receiver_v11_requires_selection_in_hello`
Expected: FAIL because v1.1 route/message handling is absent.

**Step 3: Write minimal implementation**

```rust
.route("/ws/v1.1/receivers", get(ws_receiver::ws_receiver_v11_handler))
```

In handler:
- parse `ReceiverHelloV11`
- set current selection state
- process `ReceiverSetSelection`
- send `ReceiverSelectionApplied`

**Step 4: Run test to verify it passes**

Run: `cargo test -p server --test receiver_selection_v11`
Expected: PASS for handshake/selection parsing.

**Step 5: Commit**

```bash
git add services/server/src/lib.rs services/server/src/ws_receiver.rs services/server/tests/receiver_selection_v11.rs
git commit -m "feat(server): add receiver ws v1.1 selection handshake"
```

### Task 7: Implement selection resolution, backfill, live filtering, and warn-only mismatch

**Files:**
- Modify: `services/server/src/ws_receiver.rs`
- Modify: `services/server/src/repo/events.rs`
- Modify: `services/server/src/repo/forwarder_races.rs`
- Create: `services/server/tests/receiver_selection_backfill_v11.rs`
- Create: `services/server/tests/receiver_selection_current_v11.rs`

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn remove_and_readd_epoch_resumes_from_prior_cursor() {
    // select race/all, receive + ack, remove mapping, re-add mapping
    // expect replay starts after acked seq
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p server --test receiver_selection_backfill_v11 remove_and_readd_epoch_resumes_from_prior_cursor`
Expected: FAIL because selection resolver and per-target replay are incomplete.

**Step 3: Write minimal implementation**

```rust
fn should_deliver(selection: &ResolvedSelection, stream_id: Uuid, epoch: i64) -> bool {
    selection.targets.contains(&(stream_id, epoch))
}
```

Add:
- resolver for manual/race-all/race-current targets
- replay by `(stream,epoch)` using per-epoch cursors
- dynamic refresh on mapping/current-epoch changes
- warning emission when forwarder-race and epoch-race disagree

**Step 4: Run test to verify it passes**

Run:
- `cargo test -p server --test receiver_selection_backfill_v11`
- `cargo test -p server --test receiver_selection_current_v11`

Expected: PASS for backfill/current behaviors and warn-only handling.

**Step 5: Commit**

```bash
git add services/server/src/ws_receiver.rs services/server/src/repo/events.rs services/server/src/repo/forwarder_races.rs services/server/tests/receiver_selection_backfill_v11.rs services/server/tests/receiver_selection_current_v11.rs
git commit -m "feat(server): add receiver v1.1 selection resolver and replay filtering"
```

### Task 8: Update receiver service for v1.1 selection persistence and WS messages

**Files:**
- Modify: `services/receiver/src/db.rs`
- Modify: `services/receiver/src/storage/schema.sql`
- Modify: `services/receiver/src/control_api.rs`
- Modify: `services/receiver/src/main.rs`
- Modify: `services/receiver/src/session.rs`
- Modify: `services/receiver/tests/control_api.rs`
- Modify: `services/receiver/tests/session_resume.rs`

**Step 1: Write the failing test**

```rust
#[tokio::test]
async fn selection_mode_persists_and_round_trips_via_control_api() {
    // PUT selection race/current, GET returns same selection
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p receiver --test control_api selection_mode_persists_and_round_trips_via_control_api`
Expected: FAIL because receiver profile/subscription schema has no selection model.

**Step 3: Write minimal implementation**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SelectionMode {
    Manual,
    Race { race_id: String, epoch_scope: String },
}
```

Add control API endpoints/fields for receiver selection and use v1.1 WS messages in session loop.

**Step 4: Run test to verify it passes**

Run:
- `cargo test -p receiver --test control_api`
- `cargo test -p receiver --test session_resume`

Expected: PASS for persistence and session behavior.

**Step 5: Commit**

```bash
git add services/receiver/src/db.rs services/receiver/src/storage/schema.sql services/receiver/src/control_api.rs services/receiver/src/main.rs services/receiver/src/session.rs services/receiver/tests/control_api.rs services/receiver/tests/session_resume.rs
git commit -m "feat(receiver): persist selection mode and use ws v1.1"
```

### Task 9: Add receiver UI controls for manual vs race/all/current selection

**Files:**
- Modify: `apps/receiver-ui/src/lib/api.ts`
- Modify: `apps/receiver-ui/src/lib/api.test.ts`
- Modify: `apps/receiver-ui/src/routes/+page.svelte`
- Modify: `apps/receiver-ui/src/lib/sse.ts`

**Step 1: Write the failing test**

```ts
it("sends race/current selection payload", async () => {
  await putSelection({ mode: "race", race_id: "r1", epoch_scope: "current" });
  expect(fetch).toHaveBeenCalledWith("/api/v1/selection", expect.objectContaining({ method: "PUT" }));
});
```

**Step 2: Run test to verify it fails**

Run: `cd apps/receiver-ui && npm test -- api.test.ts`
Expected: FAIL because selection API client is missing.

**Step 3: Write minimal implementation**

```ts
export type ReceiverSelection =
  | { mode: "manual" }
  | { mode: "race"; race_id: string; epoch_scope: "all" | "current" };
```

Add UI controls:
- mode toggle (manual/race)
- race selector
- epoch scope selector (`all`/`current`)

**Step 4: Run test to verify it passes**

Run:
- `cd apps/receiver-ui && npm test -- api.test.ts`
- `cd apps/receiver-ui && npm run check`

Expected: PASS.

**Step 5: Commit**

```bash
git add apps/receiver-ui/src/lib/api.ts apps/receiver-ui/src/lib/api.test.ts apps/receiver-ui/src/routes/+page.svelte apps/receiver-ui/src/lib/sse.ts
git commit -m "feat(receiver-ui): add selection controls for race epoch mode"
```

### Task 10: Add server UI controls for stream-epoch race assignment and activate-next

**Files:**
- Modify: `apps/server-ui/src/lib/api.ts`
- Modify: `apps/server-ui/src/lib/api.test.ts`
- Modify: `apps/server-ui/src/routes/streams/[streamId]/+page.svelte`
- Modify: `apps/server-ui/src/routes/streams/[streamId]/+page.ts`

**Step 1: Write the failing test**

```ts
it("calls activate-next endpoint for selected race", async () => {
  await activateNextEpoch("race-1", "stream-1");
  expect(fetch).toHaveBeenCalledWith(
    expect.stringContaining("/api/v1/races/race-1/streams/stream-1/epochs/activate-next"),
    expect.objectContaining({ method: "POST" }),
  );
});
```

**Step 2: Run test to verify it fails**

Run: `cd apps/server-ui && npm test -- api.test.ts`
Expected: FAIL because API helpers and UI actions are missing.

**Step 3: Write minimal implementation**

```ts
export async function setStreamEpochRace(streamId: string, epoch: number, raceId: string | null): Promise<void> { /* PUT */ }
export async function activateNextEpoch(raceId: string, streamId: string): Promise<void> { /* POST */ }
```

Add UI actions in stream detail page:
- assign/unassign race per epoch row
- activate-next button for selected race

**Step 4: Run test to verify it passes**

Run:
- `cd apps/server-ui && npm test -- api.test.ts`
- `cd apps/server-ui && npm run check`

Expected: PASS.

**Step 5: Commit**

```bash
git add apps/server-ui/src/lib/api.ts apps/server-ui/src/lib/api.test.ts apps/server-ui/src/routes/streams/[streamId]/+page.svelte apps/server-ui/src/routes/streams/[streamId]/+page.ts
git commit -m "feat(server-ui): add stream epoch race mapping controls"
```

### Task 11: Final integration verification and docs sync

**Files:**
- Modify: `docs/runbooks/` (only if runbook updates are needed)
- Modify: `services/server/README.md`
- Modify: `services/receiver/README.md`

**Step 1: Write failing integration assertions first (if missing)**

```rust
#[tokio::test]
async fn race_current_switches_immediately_after_activate_next() {
    // full forwarder -> server -> receiver flow
}
```

**Step 2: Run targeted test suites**

Run:
- `cargo test -p rt-protocol`
- `cargo test -p server --tests`
- `cargo test -p receiver --tests`

Expected: At least one FAIL before final glue fixes.

**Step 3: Implement minimal glue fixes**

Keep fixes small and local (routing, serialization fields, cursor query predicates, UI API shape mismatches).

**Step 4: Run full verification**

Run:
- `cargo fmt --all`
- `cargo clippy --workspace --all-targets`
- `cargo test --workspace --lib`
- `cd apps/server-ui && npm test && npm run check`
- `cd apps/receiver-ui && npm test && npm run check`

Expected: PASS.

**Step 5: Commit**

```bash
git add services/server/README.md services/receiver/README.md docs/runbooks
git commit -m "docs: update v1.1 race epoch selection runbooks"
```

## Execution Notes

- Use `@superpowers:test-driven-development` during each task.
- Use `@superpowers:verification-before-completion` before claiming completion.
- Keep existing `/ws/v1/receivers` behavior unchanged and covered by regression tests.
- Keep forwarder-race mismatch behavior warn-only; do not auto-correct mappings.

