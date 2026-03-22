# DBF File Locking Design

## Problem

The receiver's `append_record` function writes to the IPICO DBF file without OS-level file locking. Race Director (or another process) could read the file mid-write, seeing a partially written record or an inconsistent header record count. The IPICO Direct application uses a lock → read → write → unlock pattern per record; the receiver should match this behavior.

## Approach

Per-record exclusive locking in `append_record`, matching the IPICO Direct pattern observed via Process Monitor. The lock is held only for the duration of one record write — the shortest possible window — so Race Director can still read between writes.

## Dependency

Add `fs2 = "0.4"` to `services/receiver/Cargo.toml`. `fs2` provides `FileExt::lock_exclusive()` and `FileExt::unlock()`, mapping to `LockFileEx`/`UnlockFileEx` on Windows and `flock` on Unix. Cross-platform so locking works in macOS/Linux test environments too.

## Changes

### `services/receiver/src/dbf_writer.rs`

**`append_record` only.** No changes to `create_empty_dbf`, `clear_dbf`, `run_dbf_writer`, `serialize_record`, or `map_to_dbf_fields`.

Add `use fs2::FileExt;` to imports.

Current sequence:
```
open → read header → sanity check → seek → write record → update count → flush → drop
```

New sequence:
```
open → lock_exclusive → read header → sanity check → seek → write record → update count → flush → unlock → drop
```

Specifically:
1. After `OpenOptions::new().read(true).write(true).open(path)?`, call `file.lock_exclusive()?`.
2. After `file.flush()?`, call `file.unlock()?`.
3. Error paths don't need explicit unlock — dropping the file handle releases the lock on both Windows (`LockFileEx`) and Unix (`flock`).

### `services/receiver/Cargo.toml`

Add `fs2 = "0.4"` to `[dependencies]`.

## Testing

1. **Existing tests pass unchanged.** Adding locking to `append_record` doesn't change its behavior for single-threaded callers.

2. **New test: `append_record_concurrent_writers_produce_valid_file`.** Spawn two threads that each call `append_record` 50 times on the same file. After both complete, verify the file contains exactly 100 records and is readable by the `dbase` crate without corruption. Confirms the lock serializes concurrent writers.

## Non-goals

- Locking in `create_empty_dbf` or `clear_dbf` — these are user-initiated one-shot operations, not concurrent with record writes.
- Holding the lock across the writer task's lifetime — this would block Race Director from reading during an active race.
- Batching records under a single lock — IPICO Direct writes one at a time, and the throughput doesn't warrant batching.
