# Logs Descending Order Design

## Goal

Display log entries newest-first in all log viewer UIs (server-ui, receiver-ui, forwarder-ui).

## Current State

- `UiLogger` ring buffer (`rt-ui-log`) stores entries chronologically; `entries()` returns oldest-first
- REST `/api/v1/logs` returns the buffer as-is (oldest-first)
- All three UIs use the shared `LogViewer.svelte` component, which renders entries in array order
- SSE log entries are appended to the end of the list
- Reads tables already default to descending (unrelated, no changes needed)

## Design

### 1. Backend: `rt-ui-log` `entries()` returns reversed order

`entries()` will return entries newest-first. The ring buffer internals remain chronological (push_back / pop_front unchanged). Only the snapshot returned to callers is reversed.

### 2. Frontend log buffers: prepend instead of append

- `pushLog()` in `server-ui/src/lib/stores.ts`: prepend new entry, trim from end
- `pushLogEntry()` in `forwarder-ui/src/lib/log-buffer.ts`: prepend new entry, trim from end
- `receiver-ui` SSE handler: prepend new entries
- `mergeLogsWithPendingLive()` in `server-ui/src/lib/logs-merge.ts`: prepend pending live entries to front of (already-reversed) REST snapshot

### 3. `LogViewer.svelte`: auto-scroll-to-top

- If user is at the top of the list (within a small threshold), pin to top as new entries prepend
- If user has scrolled down, maintain their scroll position (shift it to account for new entries above)

### Unchanged

- `filterEntries` / `parseLogLevel` logic (order-agnostic)
- Reads tables (already default to descending)
- Ring buffer push/pop internals
