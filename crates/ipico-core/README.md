# ipico-core

Core IPICO chip-read parsing and validation library.

## Purpose

Provides the shared data types and parsing logic for IPICO timing system chip reads. The parser accepts UTF-8 strings, validates the checksum and structure, and returns typed values. Used by the forwarder, server, streamer, and emulator services.

The parser accepts both legacy ASCII `aa...FS` / `aa...LS` suffixes used in this repo's fixtures and TTO-enabled ASCII `aa` frames where the reader appends index/page/tamper bytes before the checksum.

## Key types

- **`ChipRead`** -- A parsed IPICO chip read containing `tag_id`, `timestamp`, `read_type`, and optional `tto` metadata. Implements `TryFrom<&str>` for parsing raw read lines with checksum validation.
- **`Timestamp`** -- Date-time representation with year, month, day, hour, minute, second, and millisecond fields. Supports `Display` (ISO-8601 style) and `Ord` for chronological sorting.
- **`ReadType`** -- Enum distinguishing `RAW` (streaming) reads from `FSLS` (first-seen/last-seen) reads. Implements `TryFrom<&str>` and provides `as_str()` for round-tripping.
- **`TtoInfo`** -- Optional TTO metadata decoded from TTO-enabled ASCII frames, including page/index bytes and the observed tamper / first-seen / last-seen flags.
