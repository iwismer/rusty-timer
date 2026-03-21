# IPICO Direct DBF File Format

Race Director uses a suite of "Direct" applications (IPICO Direct, MyLaps Direct,
ChronoTrack Direct, etc.) as middleware between timing hardware and the Race Director
application. IPICO Direct reads from the IPICO reader and writes chip reads to a
Visual FoxPro DBF file on disk. Race Director then imports reads from this file.

The goal of documenting this format is to allow the rusty-timer receiver to write
this file directly, eliminating the need for IPICO Direct as middleware.

## File Location

```
C:\winrace\Files\IPICO.DBF
```

This path is hardcoded — Race Director checks this location without any user
configuration. No port numbers or IP addresses are involved.

## How IPICO Direct Writes

Observed via Process Monitor (procmon), IPICO Direct performs a **lock → read →
write → unlock** sequence for each individual read it appends. It does not batch
writes.

## How Race Director Reads

Race Director performs a **single bulk read** when the user selects "Import Times"
from the Chip Results import screen. It auto-detects that IPICO Direct times exist
and prompts the user. It does not poll the file continuously.

## File Format

The file is a **Visual FoxPro DBF** (version byte `0x30`). All fields are
fixed-width character (type `C`). There are no memo fields (no `.FPT` sidecar).

### Header

| Offset | Size | Value | Description |
|--------|------|-------|-------------|
| 0 | 1 | `0x30` | Version: Visual FoxPro |
| 1–3 | 3 | YY MM DD | Last update date (binary, year offset from 2000) |
| 4–7 | 4 | LE uint32 | Number of records |
| 8–9 | 2 | `0x0248` (584) | Header size in bytes |
| 10–11 | 2 | `0x0029` (41) | Record size in bytes (1 deletion flag + 40 data) |
| 29 | 1 | `0x03` | Code page: ANSI |

Field descriptors start at offset 32, each 32 bytes. The header is padded to 584
bytes total. A terminator byte `0x0D` follows the last field descriptor.

### Record Layout

Each record is 41 bytes: a 1-byte deletion flag (space = active, `*` = deleted)
followed by 40 bytes of field data.

| Field | Type | Offset | Width | Description |
|---------|------|--------|-------|-------------|
| EVENT | C | 1 | 1 | Read type: `S` = Start, `F` = Finish |
| DIVISION | C | 2 | 2 | Division code (observed: always blank) |
| CHIP | C | 4 | 12 | IPICO chip hex ID (e.g. `058000123b32`) |
| TIME | C | 16 | 8 | Timestamp: `HHMMSSHH` (hours, minutes, seconds, hundredths) |
| RUNERNO | C | 24 | 5 | Bib/runner number (right-aligned, space-padded) |
| DAYCODE | C | 29 | 6 | Date: `YYMMDD` (e.g. `260321` = 2026-03-21) |
| LAPNO | C | 35 | 3 | Lap number (observed: always blank) |
| TPOINT | C | 38 | 2 | Timing point: matches EVENT (`S ` or `F `) |
| READER | C | 40 | 1 | Reader number (e.g. `4`) |

### Field Details

#### EVENT and TPOINT

These two fields always match in observed data:

| Value | EVENT | TPOINT | Meaning |
|-------|-------|--------|---------|
| Start | `S` | `S ` | Start line read |
| Finish | `F` | `F ` | Finish line read |

It is likely that other timing point values exist for multi-point courses (e.g.
checkpoint splits), but only `S` and `F` have been observed.

#### CHIP

The 12-character hex representation of the IPICO tag ID. This is the same format
used in the raw IPICO protocol frames (`aa` lines) and parsed by `ipico-core`.

Examples from the sample file:
- `058000123b32`
- `058000120e38`
- `058000128608`
- `058000121838`

#### TIME

8-character timestamp in `HHMMSSHH` format where `HH` at the end is hundredths of
a second.

| Characters | Meaning | Example |
|------------|---------|---------|
| 1–2 | Hours (00–23) | `12` |
| 3–4 | Minutes (00–59) | `06` |
| 5–6 | Seconds (00–59) | `50` |
| 7–8 | Hundredths (00–99) | `46` |

Example: `12065046` → `12:06:50.46`

Note: The raw IPICO protocol provides higher precision (1/256th second), but the
DBF format truncates to hundredths.

#### RUNERNO

5-character bib number, right-aligned and space-padded. This mapping (chip → bib)
is configured in IPICO Direct. If the receiver writes this file, it would either
need the chip-to-bib mapping or leave this field blank for Race Director to resolve.

#### DAYCODE

6-character date in `YYMMDD` format.

Example: `260321` → 2026-03-21 (March 21, 2026)

#### READER

Single character identifying which reader produced the read. In the sample data,
all reads come from reader `4`.

## Deduplication

IPICO Direct does **not** deduplicate reads. In the sample file, every unique read
appears exactly twice — this matches the Lite reader hardware which has two antenna
loops per mat. Each pass over the mat generates two reads with identical chip, time,
and all other fields.

Race Director handles deduplication on import.

## Sample Data

A sample DBF file is included at
[`IPICO-sample.DBF`](./IPICO-sample.DBF) containing 42 records
(21 unique reads, each duplicated):

- 4 chips across 2 timing points (Start and Finish)
- Multiple passes per chip (simulating runners crossing the mat multiple times)
- All from reader 4, single day

## Implementation Notes

### Writing from the Receiver

The `dbase` Rust crate (v0.7, `dbase-rs`) supports reading and writing Visual
FoxPro DBF files. Add to `Cargo.toml`:

```toml
dbase = { version = "0.7", features = ["serde"] }
```

The receiver would need to:

1. Create or open `C:\winrace\Files\IPICO.DBF` with the correct header and field
   descriptors
2. Append records as reads arrive, using the lock → write → unlock pattern observed
   from IPICO Direct
3. Map the raw IPICO frame fields to the DBF record format:
   - Chip hex ID → CHIP
   - Timestamp → TIME (truncate to hundredths)
   - Read type → EVENT and TPOINT
   - Date → DAYCODE
   - Reader number → READER

### Open Questions

- **RUNERNO mapping**: Does Race Director require bib numbers in the DBF, or can it
  resolve chip → bib from its own database? If the latter, the receiver can leave
  RUNERNO blank.
- **DIVISION and LAPNO**: These were always blank in the sample. Need to determine
  if Race Director ever expects values here, or if it computes laps itself.
- **Other timing points**: Only `S` (Start) and `F` (Finish) observed. Need to test
  with split/checkpoint readers to see what other TPOINT values are used.
- **File lifecycle**: Does Race Director expect the file to be cleared between
  events, or does it handle re-reading existing data? Does IPICO Direct truncate
  the file at session start?
- **Concurrent access**: Since the receiver and Race Director may access the file
  simultaneously, proper file locking (as IPICO Direct does) is important.
