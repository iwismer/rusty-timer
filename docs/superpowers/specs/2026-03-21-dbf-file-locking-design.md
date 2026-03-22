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
exists check → (create if missing) → open → read header → sanity check → seek → write record → update count → flush → drop
```

New sequence:
```
create-or-open → lock_exclusive → (init header if empty) → read header → sanity check → seek → write record → update count → flush → unlock → drop
```

Specifically:
1. Replace the `path.exists()` / `create_empty_dbf` check and separate `open()` with a single `OpenOptions::new().read(true).write(true).create(true).open(path)?`. This atomically creates the file if it doesn't exist without truncating an existing one.
2. Call `file.lock_exclusive()?` immediately after opening.
3. Check the file length (e.g., `file.metadata()?.len() == 0`). If zero, write the empty DBF header (from the embedded template) while holding the lock. This eliminates the TOCTOU race where two concurrent writers could both try to create/initialize the file.
4. After `file.flush()?`, call `file.unlock()?`. The explicit unlock is for clarity — dropping the file handle also releases the lock on both Windows (`LockFileEx`) and Unix (`flock`).
5. `lock_exclusive()` errors propagate via `?`, surfacing as write failures in `run_dbf_writer`'s existing `consecutive_failures` counter.

### `services/receiver/Cargo.toml`

Add `fs2 = "0.4"` to `[dependencies]`.

## Testing

1. **Existing tests pass unchanged.** Adding locking to `append_record` doesn't change its behavior for single-threaded callers.

2. **New test: `append_record_concurrent_writers_produce_valid_file`.** Spawn two threads that each call `append_record` 50 times on the same file, using different reader indices per thread so records are distinguishable. After both complete, verify the file contains exactly 100 records, is readable by the `dbase` crate without corruption, and each thread's records are individually correct. Confirms the lock serializes concurrent writers.

## Non-goals

- Locking in `create_empty_dbf` or `clear_dbf` — these are user-initiated one-shot operations, not concurrent with record writes. Note: on Unix, `clear_dbf` creates a new file (new inode), so an existing `flock` lock on the old inode provides no protection. On Windows, the exclusive lock blocks the overwrite, which is the safe behavior on the production platform.
- Holding the lock across the writer task's lifetime — this would block Race Director from reading during an active race.
- Batching records under a single lock — IPICO Direct writes one at a time, and the throughput doesn't warrant batching.
