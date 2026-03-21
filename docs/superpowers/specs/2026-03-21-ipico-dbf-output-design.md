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

Added via `ALTER TABLE` migration in `db.rs`, following the existing pattern for `update_mode` and `receiver_mode_json`.

### `subscriptions` table — one new column

| Column | Type | Default | Description |
|--------|------|---------|-------------|
| `event_type` | `TEXT NOT NULL` | `'finish'` | `'start'` or `'finish'` — maps to `S`/`F` in DBF |

Added via `ALTER TABLE` migration. The UI defaults to `finish` when creating a subscription.

## Architecture

### DbfWriter — event bus subscriber

A new module `services/receiver/src/dbf_writer.rs` that follows the same pattern as `LocalProxy`: subscribes to the `broadcast::Sender<ReadEvent>` event bus and processes reads independently.

**Lifecycle**:
- Started by `runtime.rs` at startup if `dbf_enabled` is true
- Subscribes to the broadcast event bus
- Stopped when `dbf_enabled` is toggled off, restarted when toggled on
- Restarted when `dbf_path` changes

**Per-ReadEvent processing**:
1. Parse raw frame to extract chip ID (reuse `chip_id_from_raw_frame`)
2. Look up the subscription's `event_type` to determine `S` or `F`
3. Extract and format timestamp as `HHMMSSHH` (truncating IPICO 1/256s precision to hundredths)
4. Extract and format date as `YYMMDD`
5. Derive READER field from last octet of `reader_ip` (single character)
6. Append one DBF record

**DBF record field mapping**:

| DBF Field | Width | Source |
|-----------|-------|--------|
| EVENT | 1 | `event_type` mapped: `start` → `S`, `finish` → `F` |
| DIVISION | 2 | Blank (space-padded) |
| CHIP | 12 | Hex chip ID from raw frame bytes 4–15 |
| TIME | 8 | `HHMMSSHH` from IPICO timestamp |
| RUNERNO | 5 | Blank (space-padded) |
| DAYCODE | 6 | `YYMMDD` from IPICO timestamp |
| LAPNO | 3 | Blank (space-padded) |
| TPOINT | 2 | Mirrors EVENT: `S ` or `F ` |
| READER | 1 | Last octet of `reader_ip` |

**File handling**:
- If the file does not exist, create it with the correct Visual FoxPro schema
- If it exists, append using `TableWriterBuilder::from_reader()` to preserve the `0x30` version byte
- File opened with exclusive access per write (matching IPICO Direct's lock-per-record pattern)

**Clear action**:
- Deletes the DBF file (or rewrites as an empty DBF with just the header)
- Next incoming read recreates the file

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

- **Field mapping unit tests**: raw frame → DBF record conversion (chip ID, timestamp `HHMMSSHH`, date `YYMMDD`, reader IP last octet, event type `S`/`F`)
- **DbfWriter unit tests**: temp file, mock ReadEvents through broadcast channel, verify DBF contents
- **Clear test**: write records, clear, verify empty, verify new writes create fresh file
- **Enable/disable test**: no writes when disabled, writes resume when enabled
- **Sample file compatibility**: verify written DBF can be read back with correct field values

## Out of Scope

- Bib number population (RUNERNO left blank)
- Automatic clear on startup
- Per-subscription DBF files
- Deduplication (Race Director handles this on import)
