# Show Current Seqnum in Admin Cursor Reset Table - Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Show the current cursor epoch and sequence number in the admin page's Cursor Reset table so operators can see stream progress before resetting.

**Architecture:** Enrich `GET /api/v1/streams` by joining cursor data from SQLite into each `StreamEntry`. Frontend adds two columns to the Cursor Reset table. Field names use `cursor_epoch` / `cursor_seq` to avoid collision with the existing `stream_epoch` field (which comes from the upstream server).

**Tech Stack:** Rust/Axum backend, SvelteKit frontend (Svelte 5 runes)

---

### Task 1: Add cursor fields to StreamEntry and populate them

**Files:**
- Modify: `services/receiver/src/control_api.rs:364-382` (StreamEntry struct)
- Modify: `services/receiver/src/control_api.rs:175-276` (build_streams_response)

**Step 1: Add `cursor_epoch` and `cursor_seq` to `StreamEntry`**

In the `StreamEntry` struct at line 364, add two new optional fields after `reads_epoch`:

```rust
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_epoch: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_seq: Option<u64>,
```

**Step 2: Load cursors in `build_streams_response` and build a lookup map**

In `build_streams_response` at line 177, after loading subscriptions and before dropping db, also load cursors:

```rust
        let cursors = match db.load_cursors() {
            Ok(c) => c,
            Err(_) => vec![],
        };
```

Then after `drop(db);` (line 188), build the cursor lookup map:

```rust
        let cursor_map: HashMap<(&str, &str), &crate::db::CursorRecord> = cursors
            .iter()
            .map(|c| ((c.forwarder_id.as_str(), c.reader_ip.as_str()), c))
            .collect();
```

**Step 3: Attach cursor data to each StreamEntry**

In both places where `StreamEntry` is constructed (lines 230 and 256), look up the cursor and add the fields. Before each `streams.push(StreamEntry { ... })`, add:

```rust
                let cursor = cursor_map.get(&(si.forwarder_id.as_str(), si.reader_ip.as_str()));
```

(Use `sub.forwarder_id` / `sub.reader_ip` for the second block.)

Then in each `StreamEntry` literal, add:

```rust
                    cursor_epoch: cursor.map(|c| c.stream_epoch),
                    cursor_seq: cursor.map(|c| c.last_seq),
```

**Step 4: Verify it compiles**

Run: `cargo check -p receiver`
Expected: compiles with no errors

**Step 5: Commit**

```
feat(receiver): add cursor_epoch and cursor_seq to streams response
```

---

### Task 2: Add integration test for cursor fields in streams response

**Files:**
- Modify: `services/receiver/tests/control_api.rs`

**Step 1: Write test for streams response including cursor data**

Add a new test at the end of the file:

```rust
#[tokio::test]
async fn streams_response_includes_cursor_data() {
    let (app, state) = setup_with_state();
    {
        let db = state.db.lock().await;
        db.save_subscription("f1", "10.0.0.1", None).unwrap();
        db.save_subscription("f2", "10.0.0.2", None).unwrap();
        db.save_cursor("f1", "10.0.0.1", 5, 42).unwrap();
        // f2 has no cursor — fields should be absent
    }
    let (status, body) = get_json(app, "/api/v1/streams").await;
    assert_eq!(status, StatusCode::OK);
    let streams = body["streams"].as_array().unwrap();
    assert_eq!(streams.len(), 2);

    // Find f1 — should have cursor data
    let f1 = streams.iter().find(|s| s["forwarder_id"] == "f1").unwrap();
    assert_eq!(f1["cursor_epoch"], 5);
    assert_eq!(f1["cursor_seq"], 42);

    // Find f2 — should have no cursor fields (skip_serializing_if)
    let f2 = streams.iter().find(|s| s["forwarder_id"] == "f2").unwrap();
    assert!(f2.get("cursor_epoch").is_none() || f2["cursor_epoch"].is_null());
    assert!(f2.get("cursor_seq").is_none() || f2["cursor_seq"].is_null());
}
```

**Step 2: Run the test**

Run: `cargo test -p receiver --test control_api streams_response_includes_cursor_data`
Expected: PASS

**Step 3: Commit**

```
test(receiver): verify cursor data in streams response
```

---

### Task 3: Add cursor columns to admin page frontend

**Files:**
- Modify: `apps/receiver-ui/src/lib/api.ts:14-26` (StreamEntry type)
- Modify: `apps/receiver-ui/src/routes/admin/+page.svelte:240-285` (Cursor Reset table)

**Step 1: Add cursor fields to the TypeScript StreamEntry type**

In `api.ts`, add to the `StreamEntry` interface after `reads_epoch`:

```typescript
  cursor_epoch?: number;
  cursor_seq?: number;
```

**Step 2: Add "Epoch" and "Seq" column headers to the Cursor Reset table**

In `+page.svelte`, in the `<thead>` of the Cursor Reset table (around line 242-247), add two columns between "Reader" and the empty action column:

```svelte
              <th class="py-2 pr-4 font-medium">Epoch</th>
              <th class="py-2 pr-4 font-medium">Seq</th>
```

**Step 3: Add the data cells in each row**

In the `<tbody>` `{#each}` block (around line 250-283), add two `<td>` elements between the Reader `<td>` and the action button `<td>`:

```svelte
                <td class="py-2 pr-4 text-text-secondary tabular-nums"
                  >{stream.cursor_epoch ?? "\u2014"}</td
                >
                <td class="py-2 pr-4 text-text-secondary tabular-nums"
                  >{stream.cursor_seq ?? "\u2014"}</td
                >
```

The `\u2014` is an em dash, shown when no cursor exists.

**Step 4: Verify the frontend builds**

Run: `cd apps/receiver-ui && npm run check`
Expected: no errors

**Step 5: Commit**

```
feat(receiver-ui): show cursor epoch and seq in admin reset table
```
