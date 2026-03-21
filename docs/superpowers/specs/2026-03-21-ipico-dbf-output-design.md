# IPICO DBF Output for Receiver

**Date**: 2026-03-21
**Status**: Draft
**Component**: `services/receiver`

## Overview

Add the ability for the receiver to write incoming chip reads to an IPICO-compatible DBF file (`C:\winrace\Files\IPICO.DBF` by default), so Race Director can import timing data directly without IPICO Direct middleware.

## Requirements

- DBF output is configurable: enable/disable toggle and file path setting
- Each subscription has a required `event_type` (`start` or `finish`) to populate the DBF EVENT field
- A manual "Clear DBF" action wipes the file so Race Director doesn't re-import stale data
- The DBF file format matches the IPICO Direct Visual FoxPro DBF spec documented in `docs/race-director/ipico-direct-dbf-format.md`

## Schema Changes

### `profile` table — two new columns

| Column | Type | Default | Description |
|--------|------|---------|-------------|
| `dbf_enabled` | `INTEGER NOT NULL` | `0` | 0 = off, 1 = on |
| `dbf_path` | `TEXT NOT NULL` | `C:\winrace\Files\IPICO.DBF` | Path to the output DBF file |

Added via `ALTER TABLE` migration in `db.rs`, following the existing pattern for `update_mode` and `receiver_mode_json`. The `save_profile()` function must be updated to include these columns in its DELETE+INSERT cycle.

### `subscriptions` table — one new column

| Column | Type | Default | Description |
|--------|------|---------|-------------|
| `event_type` | `TEXT NOT NULL` | `'finish'` | `'start'` or `'finish'` — maps to `S`/`F` in DBF |

Added via `ALTER TABLE` migration. The `Subscription` struct, `save_subscription`, `replace_subscriptions`, and `load_subscriptions` must all be updated to include this field. The UI defaults to `finish` when creating a subscription.

## Architecture

### Global broadcast channel

The existing `EventBus` maintains per-stream broadcast channels keyed by `(forwarder_id, reader_ip)`. The DbfWriter needs reads from **all** subscribed streams in a single consumer.

Add a new global `broadcast::Sender<ReadEvent>` to the receiver runtime. The session loop publishes each incoming read to both the per-stream channel (for `LocalProxy`) and the global channel (for `DbfWriter`). This is lightweight — just a clone of the `ReadEvent` per message.

### ipico-core: add public timestamp accessors

The `Timestamp` struct in `ipico-core` currently has private fields. Add public accessor methods (`hour()`, `minute()`, `second()`, `millis()`, `year()`, `month()`, `day()`) so the DbfWriter can format timestamps without duplicating parsing logic.

### DbfWriter — global event bus subscriber

A new module `services/receiver/src/dbf_writer.rs` that subscribes to the global broadcast channel.

**Lifecycle**:
- Started by `runtime.rs` at startup if `dbf_enabled` is true
- Subscribes to the global broadcast channel
- Stopped when `dbf_enabled` is toggled off, restarted when toggled on
- Restarted when `dbf_path` changes

**Per-ReadEvent processing**:
1. Skip sentinel read types (where `read_type` starts with `__`)
2. Parse raw frame to extract chip ID (reuse `chip_id_from_raw_frame`)
3. Look up the subscription's `event_type` to determine `S` or `F`
4. Extract timestamp via `Timestamp` public accessors, format as `HHMMSSHH`
5. Extract date via `Timestamp` public accessors, format as `YYMMDD`
6. Derive READER field: readers are numbered 0–9 in subscription order; reads from readers beyond index 9 are **not written** to the DBF file
7. Append one DBF record

**DBF record field mapping**:

| DBF Field | Width | Source |
|-----------|-------|--------|
| EVENT | 1 | `event_type` mapped: `start` → `S`, `finish` → `F` |
| DIVISION | 2 | Blank (space-padded) |
| CHIP | 12 | Hex chip ID from raw frame characters at positions 4..16 |
| TIME | 8 | `HHMMSSHH` from IPICO `Timestamp` accessors |
| RUNERNO | 5 | Blank (space-padded) |
| DAYCODE | 6 | `YYMMDD` from IPICO `Timestamp` accessors |
| LAPNO | 3 | Blank (space-padded) |
| TPOINT | 2 | Mirrors EVENT: `S ` or `F ` |
| READER | 1 | Reader index 0–9 (subscription order) |

**File handling**:
- If the file does not exist, create it with the correct Visual FoxPro schema. Use an embedded template header byte array (with version byte `0x30`) since `TableWriterBuilder::new()` creates dBase III (`0x03`) by default
- If it exists, append using `TableWriterBuilder::from_reader()` to preserve the existing header
- File opened with exclusive access per write (matching IPICO Direct's lock-per-record pattern)

**Error handling**:
- Write failures (disk full, file locked by Race Director, invalid path) are logged and the read is skipped — no retry, no buffering
- A persistent error counter is exposed via the control API so the UI can surface write failures

**Clear action**:
- Rewrites the DBF file as an empty file with just the Visual FoxPro header (no records)
- This avoids a "file not found" window if Race Director reads during the clear
- Next incoming read appends to the existing empty file

### Dependency

Add to `services/receiver/Cargo.toml`:
```toml
dbase = { version = "0.7", features = ["serde", "yore"] }
```

The `yore` feature is required because the code page byte `0x03` (Windows-1252/ANSI) is unsupported without it.

## Control API

New endpoints on the existing `127.0.0.1:9090` control API:

| Method | Path | Body | Description |
|--------|------|------|-------------|
| `GET` | `/config/dbf` | — | Returns `{ enabled, path }` |
| `PUT` | `/config/dbf` | `{ "enabled": bool, "path": string }` | Update DBF settings |
| `POST` | `/config/dbf/clear` | — | Clear the DBF file |
| `PUT` | `/subscriptions/:forwarder_id/:reader_ip/event-type` | `{ "event_type": "start" \| "finish" }` | Set event type for a subscription |

**Runtime behavior on config change**:
- `dbf_enabled` toggled on → start DbfWriter task
- `dbf_enabled` toggled off → stop DbfWriter task
- `dbf_path` changed → stop and restart DbfWriter with new path

## Tauri UI

- Settings section: toggle for DBF output, file path input with file picker, "Clear DBF File" button
- Subscription list: start/finish selector per reader (defaulting to finish)

## Testing

- **Field mapping unit tests**: raw frame → DBF record conversion (chip ID, timestamp `HHMMSSHH`, date `YYMMDD`, reader index, event type `S`/`F`)
- **Timestamp accessor tests**: verify new public accessors on `Timestamp` return correct values
- **DbfWriter unit tests**: temp file, mock ReadEvents through broadcast channel, verify DBF contents
- **Clear test**: write records, clear, verify file has header but no records, verify new writes append correctly
- **Enable/disable test**: no writes when disabled, writes resume when enabled
- **Sentinel filtering test**: verify `__`-prefixed read types are not written to DBF
- **Reader limit test**: verify reads from reader index > 9 are skipped
- **Sample file compatibility**: verify written DBF can be read back with correct field values
- **Error handling test**: verify write failures are logged and skipped without crashing

## Out of Scope

- Bib number population (RUNERNO left blank)
- Automatic clear on startup
- Per-subscription DBF files
- Deduplication (Race Director handles this on import)
