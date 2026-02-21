# ipico-core

Core IPICO chip-read parsing and validation library.

## Purpose

Provides the shared data types and parsing logic for IPICO timing system chip reads. The parser accepts UTF-8 strings, validates the checksum and structure, and returns typed values. Used by the forwarder, server, streamer, and emulator services.

## Key types

- **`ChipRead`** -- A parsed IPICO chip read containing `tag_id`, `timestamp`, and `read_type`. Implements `TryFrom<&str>` for parsing raw read lines with checksum validation.
- **`Timestamp`** -- Date-time representation with year, month, day, hour, minute, second, and millisecond fields. Supports `Display` (ISO-8601 style) and `Ord` for chronological sorting.
- **`ReadType`** -- Enum distinguishing `RAW` (streaming) reads from `FSLS` (first-seen/last-seen) reads. Implements `TryFrom<&str>` and provides `as_str()` for round-tripping.
