# Logs Descending Order Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Display log entries newest-first in all log viewer UIs (server-ui, receiver-ui, forwarder-ui).

**Architecture:** Reverse at the data layer so arrays always arrive newest-first. Backend `entries()` returns reversed snapshot. Frontend buffers prepend new SSE entries. `LogViewer.svelte` pins scroll to top when user is already there.

**Tech Stack:** Rust (rt-ui-log crate), SvelteKit 5 (Svelte 5 runes), TypeScript, Vitest

---

### Task 1: Reverse backend `entries()` return order

**Files:**
- Modify: `crates/rt-ui-log/src/lib.rs:124-132` (`entries()` method)
- Test: `crates/rt-ui-log/src/lib.rs` (existing tests in same file)

**Step 1: Update the failing tests to expect reversed order**

In `crates/rt-ui-log/src/lib.rs`, update the two tests that assert on entry order:

`log_buffers_entries` test (line ~179-189) — change to:
```rust
    #[test]
    fn log_buffers_entries() {
        let (tx, _) = broadcast::channel::<String>(4);
        let logger = UiLogger::with_buffer(tx, |entry| entry, 3);
        logger.log("a");
        logger.log("b");
        logger.log("c");
        logger.log("d");
        let entries = logger.entries();
        assert_eq!(entries.len(), 3);
        assert!(entries[0].ends_with(" d"));
        assert!(entries[2].ends_with(" b"));
    }
```

`log_at_buffers_entries` test (line ~192-203) — change to:
```rust
    #[test]
    fn log_at_buffers_entries() {
        let (tx, _) = broadcast::channel::<String>(4);
        let logger = UiLogger::with_buffer(tx, |entry| entry, 2);
        logger.log_at(UiLogLevel::Warn, "w");
        logger.log_at(UiLogLevel::Debug, "d");
        logger.log_at(UiLogLevel::Error, "e");
        let entries = logger.entries();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].contains("[ERROR]"));
        assert!(entries[1].contains("[DEBUG]"));
    }
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p rt-ui-log`
Expected: 2 FAIL (`log_buffers_entries`, `log_at_buffers_entries`)

**Step 3: Reverse the `entries()` method**

In `crates/rt-ui-log/src/lib.rs`, change `entries()`:

```rust
    pub fn entries(&self) -> Vec<String> {
        match &self.buffer {
            Some(buf) => buf
                .read()
                .map(|b| b.iter().rev().cloned().collect())
                .unwrap_or_default(),
            None => Vec::new(),
        }
    }
```

**Step 4: Run tests to verify they pass**

Run: `cargo test -p rt-ui-log`
Expected: all PASS

**Step 5: Commit**

```bash
git add crates/rt-ui-log/src/lib.rs
git commit -m "feat(rt-ui-log): return entries newest-first from entries()"
```

---

### Task 2: Prepend in `pushLogEntry` (forwarder-ui)

**Files:**
- Modify: `apps/forwarder-ui/src/lib/log-buffer.ts`
- Test: `apps/forwarder-ui/src/lib/log-buffer.test.ts`

**Step 1: Update tests to expect prepend behavior**

In `apps/forwarder-ui/src/lib/log-buffer.test.ts`, replace entire file:

```typescript
import { describe, expect, it } from "vitest";
import { pushLogEntry } from "./log-buffer";

describe("pushLogEntry", () => {
  it("prepends a new entry", () => {
    expect(pushLogEntry([], "first", 5)).toEqual(["first"]);
  });

  it("prepends newest entry to front", () => {
    expect(pushLogEntry(["b", "a"], "c", 5)).toEqual(["c", "b", "a"]);
  });

  it("keeps only latest max entries, trimming from end", () => {
    expect(pushLogEntry(["c", "b", "a"], "d", 3)).toEqual(["d", "c", "b"]);
  });

  it("trims whitespace-only entries", () => {
    expect(pushLogEntry(["a"], "   ", 5)).toEqual(["a"]);
  });

  it("preserves 500-entry retention after initial snapshot", () => {
    const initial = Array.from({ length: 500 }, (_, i) => `e-${i}`);
    const next = pushLogEntry(initial, "live");
    expect(next).toHaveLength(500);
    expect(next[0]).toBe("live");
  });
});
```

**Step 2: Run tests to verify they fail**

Run: `cd apps/forwarder-ui && npx vitest run src/lib/log-buffer.test.ts`
Expected: FAIL

**Step 3: Update `pushLogEntry` to prepend**

In `apps/forwarder-ui/src/lib/log-buffer.ts`, replace entire file:

```typescript
export function pushLogEntry(
  entries: string[],
  next: string,
  maxEntries = 500,
): string[] {
  const normalized = next.trim();
  if (!normalized) return entries;
  const prepended = [normalized, ...entries];
  if (prepended.length <= maxEntries) return prepended;
  return prepended.slice(0, maxEntries);
}
```

**Step 4: Run tests to verify they pass**

Run: `cd apps/forwarder-ui && npx vitest run src/lib/log-buffer.test.ts`
Expected: all PASS

**Step 5: Commit**

```bash
git add apps/forwarder-ui/src/lib/log-buffer.ts apps/forwarder-ui/src/lib/log-buffer.test.ts
git commit -m "feat(forwarder-ui): prepend log entries for newest-first order"
```

---

### Task 3: Prepend in `pushLog` (server-ui stores)

**Files:**
- Modify: `apps/server-ui/src/lib/stores.ts:24-29` (`pushLog` function)

**Step 1: Update `pushLog` to prepend**

In `apps/server-ui/src/lib/stores.ts`, change the `pushLog` function:

```typescript
export function pushLog(entry: string): void {
  logsStore.update((entries) => {
    const next = [entry.trim(), ...entries];
    return next.length <= 500 ? next : next.slice(0, 500);
  });
}
```

**Step 2: Commit**

```bash
git add apps/server-ui/src/lib/stores.ts
git commit -m "feat(server-ui): prepend log entries for newest-first order"
```

---

### Task 4: Update `mergeLogsWithPendingLive` (server-ui)

**Files:**
- Modify: `apps/server-ui/src/lib/logs-merge.ts`
- Test: `apps/server-ui/src/lib/logs-merge.test.ts`

**Step 1: Update tests for descending merge**

The REST snapshot now arrives newest-first. Pending live entries (collected during resync) should be prepended to the front (they are newer than anything in the snapshot).

In `apps/server-ui/src/lib/logs-merge.test.ts`, replace entire file:

```typescript
import { describe, expect, it } from "vitest";
import { mergeLogsWithPendingLive } from "./logs-merge";

describe("mergeLogsWithPendingLive", () => {
  it("prepends live entries emitted during in-flight resync", () => {
    const snapshot = ["b", "a"];
    const pending = ["c"];
    expect(mergeLogsWithPendingLive(snapshot, pending, 500)).toEqual([
      "c",
      "b",
      "a",
    ]);
  });

  it("does not duplicate entries already in snapshot", () => {
    const snapshot = ["b", "a"];
    const pending = ["b", "c"];
    expect(mergeLogsWithPendingLive(snapshot, pending, 500)).toEqual([
      "c",
      "b",
      "a",
    ]);
  });

  it("enforces max retention trimming from end", () => {
    const snapshot = Array.from({ length: 500 }, (_, i) => `s-${i}`);
    const pending = ["live-1", "live-2"];
    const merged = mergeLogsWithPendingLive(snapshot, pending, 500);
    expect(merged).toHaveLength(500);
    expect(merged[0]).toBe("live-2");
    expect(merged[1]).toBe("live-1");
  });
});
```

**Step 2: Run tests to verify they fail**

Run: `cd apps/server-ui && npx vitest run src/lib/logs-merge.test.ts`
Expected: FAIL

**Step 3: Update `mergeLogsWithPendingLive` for descending order**

In `apps/server-ui/src/lib/logs-merge.ts`, replace entire file:

```typescript
export function mergeLogsWithPendingLive(
  snapshot: string[],
  pendingLive: string[],
  maxEntries = 500,
): string[] {
  const newEntries = pendingLive.filter((entry) => !snapshot.includes(entry));
  const merged = [...newEntries.reverse(), ...snapshot];
  return merged.length <= maxEntries ? merged : merged.slice(0, maxEntries);
}
```

Note: `pendingLive` entries are collected chronologically (oldest-first as they arrive via SSE). We reverse them so the newest pending entry is first, then prepend to the snapshot (which is already newest-first from the backend).

**Step 4: Run tests to verify they pass**

Run: `cd apps/server-ui && npx vitest run src/lib/logs-merge.test.ts`
Expected: all PASS

**Step 5: Commit**

```bash
git add apps/server-ui/src/lib/logs-merge.ts apps/server-ui/src/lib/logs-merge.test.ts
git commit -m "feat(server-ui): merge pending logs in descending order"
```

---

### Task 5: Prepend in receiver-ui SSE handler

**Files:**
- Modify: `apps/receiver-ui/src/routes/+page.svelte:875-880` (onLogEntry callback)

**Step 1: Update the `onLogEntry` callback to prepend**

In `apps/receiver-ui/src/routes/+page.svelte`, change the onLogEntry handler:

```typescript
      onLogEntry: (entry) => {
        if (logs) {
          logs = { entries: [entry, ...logs.entries] };
        } else {
          logs = { entries: [entry] };
        }
      },
```

**Step 2: Commit**

```bash
git add apps/receiver-ui/src/routes/+page.svelte
git commit -m "feat(receiver-ui): prepend log entries for newest-first order"
```

---

### Task 6: Auto-scroll-to-top in `LogViewer.svelte`

**Files:**
- Modify: `apps/shared-ui/src/components/LogViewer.svelte`

**Step 1: Add scroll-to-top pinning**

The `<ul>` element needs a `bind:this` reference. When new entries prepend, if the user is scrolled to the top (scrollTop near 0), keep them pinned there. Otherwise, preserve their scroll position by adjusting for the height of newly inserted entries.

In `apps/shared-ui/src/components/LogViewer.svelte`, replace the entire file:

```svelte
<script lang="ts">
  import { tick } from "svelte";
  import {
    LOG_LEVELS,
    type LogLevel,
    parseLogLevel,
    filterEntries,
  } from "../lib/log-filter";

  let {
    entries = [],
    maxHeight = "300px",
  }: {
    entries?: string[];
    maxHeight?: string;
  } = $props();

  let selectedLevel = $state<LogLevel>("info");
  let listEl: HTMLUListElement | undefined = $state();

  let filteredEntries = $derived(filterEntries(entries, selectedLevel));

  let prevCount = 0;

  $effect(() => {
    const count = filteredEntries.length;
    const added = count - prevCount;
    if (added > 0 && listEl) {
      const wasAtTop = listEl.scrollTop < 8;
      const oldScrollTop = listEl.scrollTop;
      const oldScrollHeight = listEl.scrollHeight;
      tick().then(() => {
        if (!listEl) return;
        if (wasAtTop) {
          listEl.scrollTop = 0;
        } else {
          const heightDiff = listEl.scrollHeight - oldScrollHeight;
          listEl.scrollTop = oldScrollTop + heightDiff;
        }
      });
    }
    prevCount = count;
  });

  function levelColor(level: LogLevel): string {
    switch (level) {
      case "error":
        return "text-status-err";
      case "warn":
        return "text-status-warn";
      case "debug":
      case "trace":
        return "text-text-muted";
      default:
        return "text-text-secondary";
    }
  }
</script>

<section data-testid="logs-section">
  <div
    class="flex items-center justify-between px-4 py-2 border-b border-border"
  >
    <h2 class="text-sm font-semibold text-text-primary m-0">Logs</h2>
    <div class="flex items-center gap-3">
      <label class="flex items-center gap-1.5 text-xs text-text-muted">
        Level
        <select
          data-testid="log-level-select"
          class="text-xs bg-surface-0 border border-border rounded px-1.5 py-0.5 text-text-primary"
          bind:value={selectedLevel}
        >
          {#each LOG_LEVELS as level}
            <option value={level}>{level.toUpperCase()}</option>
          {/each}
        </select>
      </label>
      <span class="text-xs text-text-muted">
        {filteredEntries.length} / {entries.length}
      </span>
    </div>
  </div>
  {#if filteredEntries.length === 0}
    <p class="px-4 py-6 text-sm text-text-muted text-center m-0">
      No log entries.
    </p>
  {:else}
    <ul
      bind:this={listEl}
      class="font-mono text-xs overflow-y-auto list-none p-0 m-0"
      style="max-height: {maxHeight}"
    >
      {#each filteredEntries as entry}
        <li
          class="px-4 py-1 border-b border-border {levelColor(parseLogLevel(entry))}"
        >
          {entry}
        </li>
      {/each}
    </ul>
  {/if}
</section>
```

**Step 2: Commit**

```bash
git add apps/shared-ui/src/components/LogViewer.svelte
git commit -m "feat(shared-ui): auto-scroll-to-top for newest-first log viewer"
```

---

### Task 7: Smoke test all three UIs

**Step 1: Run all frontend test suites**

```bash
cd apps/forwarder-ui && npx vitest run
cd apps/server-ui && npx vitest run
cd apps/shared-ui && npx vitest run
```

Expected: all PASS

**Step 2: Run Rust tests**

```bash
cargo test -p rt-ui-log
```

Expected: all PASS

**Step 3: Commit (if any fixups needed)**

Only commit if fixes were required. Otherwise, done.
