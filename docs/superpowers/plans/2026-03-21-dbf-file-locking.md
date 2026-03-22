# DBF File Locking Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add per-record OS-level file locking to the DBF writer so Race Director cannot read a partially-written record.

**Architecture:** Wrap `append_record`'s I/O in `fs2::FileExt::lock_exclusive()` / `unlock()`. Replace the TOCTOU-vulnerable `path.exists()` + `create_empty_dbf()` with `OpenOptions::create(true)` + init-under-lock. No changes to any other function or module.

**Tech Stack:** `fs2 = "0.4"` (maps to `LockFileEx` on Windows, `flock` on Unix)

**Spec:** `docs/superpowers/specs/2026-03-21-dbf-file-locking-design.md`

---

### Task 1: Add `fs2` dependency

**Files:**
- Modify: `services/receiver/Cargo.toml:31` (after the `dbase` line)

- [ ] **Step 1: Add the dependency**

Add after the `dbase` line in `[dependencies]`:

```toml
fs2 = "0.4"
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p receiver`
Expected: compiles with no errors

- [ ] **Step 3: Commit**

```bash
git add services/receiver/Cargo.toml Cargo.lock
git commit -m "feat(receiver): add fs2 dependency for DBF file locking"
```

---

### Task 2: Add `write_empty_header` helper and refactor `append_record`

**Files:**
- Modify: `services/receiver/src/dbf_writer.rs:10-18` (imports), `services/receiver/src/dbf_writer.rs:240-286` (`append_record`)

- [ ] **Step 1: Write failing test for lock-under-creation (concurrent writers on non-existent file)**

Add to the `tests` module at the bottom of `dbf_writer.rs`:

```rust
#[test]
fn append_record_concurrent_writers_produce_valid_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("concurrent.dbf");

    // Do NOT pre-create the file — both threads race to create it
    let path1 = path.clone();
    let path2 = path.clone();

    let raw = sample_raw_frame();
    let rec_a = map_to_dbf_fields(&raw, EventType::Start, 1).unwrap();
    let rec_b = map_to_dbf_fields(&raw, EventType::Finish, 2).unwrap();

    std::thread::scope(|s| {
        s.spawn(|| {
            for _ in 0..50 {
                append_record(&path1, &rec_a).unwrap();
            }
        });
        s.spawn(|| {
            for _ in 0..50 {
                append_record(&path2, &rec_b).unwrap();
            }
        });
    });

    let mut reader = dbase::Reader::from_path(&path).unwrap();
    let records: Vec<dbase::Record> = reader.read().unwrap();
    assert_eq!(records.len(), 100, "should have exactly 100 records");

    // Verify each record is intact (not interleaved)
    let mut start_count = 0;
    let mut finish_count = 0;
    for r in &records {
        match r.get("EVENT") {
            Some(dbase::FieldValue::Character(Some(s))) => match s.trim() {
                "S" => {
                    start_count += 1;
                    // Reader index for Start records should be "1"
                    if let Some(dbase::FieldValue::Character(Some(rd))) = r.get("READER") {
                        assert_eq!(rd.trim(), "1");
                    }
                }
                "F" => {
                    finish_count += 1;
                    // Reader index for Finish records should be "2"
                    if let Some(dbase::FieldValue::Character(Some(rd))) = r.get("READER") {
                        assert_eq!(rd.trim(), "2");
                    }
                }
                other => panic!("unexpected EVENT value: {other}"),
            },
            other => panic!("unexpected EVENT field: {other:?}"),
        }
    }
    assert_eq!(start_count, 50);
    assert_eq!(finish_count, 50);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p receiver append_record_concurrent_writers -- --nocapture`
Expected: FAIL — the current code has no locking, so concurrent writers will corrupt the file (wrong record count, interleaved bytes, or `dbase` read error).

- [ ] **Step 3: Add `fs2` import and `write_empty_header` helper**

Add `use fs2::FileExt;` to the imports at the top of `dbf_writer.rs` (after line 11):

```rust
use fs2::FileExt;
```

Add this helper function before `append_record` (after `create_empty_dbf`, around line 233):

```rust
/// Write an empty Visual FoxPro DBF header to an already-open file.
///
/// Used to initialize a newly-created file while holding an exclusive lock,
/// avoiding the TOCTOU race of check-then-create.
fn write_empty_header(file: &mut std::fs::File) -> std::io::Result<()> {
    let header_size =
        u16::from_le_bytes([DBF_TEMPLATE_BYTES[8], DBF_TEMPLATE_BYTES[9]]) as usize;
    let mut header = DBF_TEMPLATE_BYTES[..header_size].to_vec();
    // Zero the record count (bytes 4-7)
    header[4..8].copy_from_slice(&0u32.to_le_bytes());
    file.write_all(&header)?;
    file.write_all(&[DBF_EOF_MARKER])?;
    file.flush()?;
    file.seek(SeekFrom::Start(0))?;
    Ok(())
}
```

- [ ] **Step 4: Refactor `append_record` with locking and init-under-lock**

Replace the entire `append_record` function body with:

```rust
pub fn append_record(path: &Path, record: &DbfRecord) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;

    file.lock_exclusive()?;

    // If the file was just created (empty), write the DBF header under the lock
    if file.metadata()?.len() == 0 {
        write_empty_header(&mut file)?;
    }

    // Read header fields: record_count (bytes 4-7), header_size (bytes 8-9),
    // record_size (bytes 10-11), all little-endian.
    let mut header_buf = [0u8; 12];
    file.read_exact(&mut header_buf)?;
    let record_count =
        u32::from_le_bytes([header_buf[4], header_buf[5], header_buf[6], header_buf[7]]);
    let header_size = u16::from_le_bytes([header_buf[8], header_buf[9]]) as u64;
    let record_size = u16::from_le_bytes([header_buf[10], header_buf[11]]) as u64;

    // Sanity check: record_size should be 1 (deletion flag) + RECORD_DATA_LEN
    if record_size != (1 + RECORD_DATA_LEN as u64) {
        return Err(std::io::Error::other(format!(
            "unexpected DBF record size: expected {}, got {record_size}",
            1 + RECORD_DATA_LEN
        )));
    }

    // Seek to where the new record should go: after all existing records
    let write_pos = header_size + (record_count as u64) * record_size;
    file.seek(SeekFrom::Start(write_pos))?;

    // Write: deletion flag + record data + EOF marker
    let record_bytes = serialize_record(record);
    file.write_all(&[DBF_RECORD_NOT_DELETED])?;
    file.write_all(&record_bytes)?;
    file.write_all(&[DBF_EOF_MARKER])?;

    // Update record count in header (bytes 4-7)
    let new_count = record_count
        .checked_add(1)
        .ok_or_else(|| std::io::Error::other("DBF record count overflow"))?;
    file.seek(SeekFrom::Start(4))?;
    file.write_all(&new_count.to_le_bytes())?;

    file.flush()?;
    file.unlock()?;
    Ok(())
}
```

- [ ] **Step 5: Run all receiver tests**

Run: `cargo test -p receiver`
Expected: ALL tests pass, including the new concurrent writers test and all existing tests (`create_and_append_dbf_records`, `append_record_auto_creates_file`, `append_multiple_records_increments_count`, `clear_dbf_removes_records`, `created_dbf_uses_visual_foxpro_header`, `cleared_dbf_preserves_visual_foxpro_header`, `serialize_record_produces_correct_bytes`, `map_to_dbf_fields_*`, `read_sample_dbf_file`, `written_dbf_has_same_fields_as_sample`, `dbf_writer_*`).

- [ ] **Step 6: Commit**

```bash
git add services/receiver/src/dbf_writer.rs
git commit -m "feat(receiver): add per-record file locking to DBF writer

Use fs2::lock_exclusive()/unlock() around the append_record I/O to
match IPICO Direct's lock-write-unlock pattern. Replace the TOCTOU-
vulnerable exists()+create check with OpenOptions::create(true) and
init-under-lock. Adds concurrent-writers test."
```
