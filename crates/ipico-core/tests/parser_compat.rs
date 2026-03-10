//! Compatibility tests for IPICO read parsing.
//!
//! These tests verify that the extracted `ipico_core` parser produces
//! identical results to the original `src/models/chip.rs` implementation.
//! Fixture files live in `tests/fixtures/` — each line is one raw IPICO
//! read string (valid UTF-8, no trailing whitespace beyond the newline).

use std::convert::TryFrom;

use ipico_core::read::{ChipRead, ReadType, Timestamp, TtoInfo};

// ---------------------------------------------------------------------------
// Helper: load non-empty lines from a fixture file
// ---------------------------------------------------------------------------
fn fixture_lines(name: &str) -> Vec<String> {
    let path = format!("{}/tests/fixtures/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read fixture {}: {}", path, e))
        .lines()
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

fn raw_read_with_tto_and_checksum(tto_hex: &str) -> String {
    let body = format!("400000000123450a2a01123018455927{}", tto_hex);
    let checksum: u8 = body.bytes().map(|b| b as u32).sum::<u32>() as u8;
    format!("aa{}{:02x}", body, checksum)
}

// ===========================================================================
// RAW fixture tests
// ===========================================================================

#[test]
fn raw_fixture_parses_all_lines() {
    let lines = fixture_lines("raw_reads.txt");
    assert_eq!(lines.len(), 3, "expected 3 RAW fixture lines");
    for (i, line) in lines.iter().enumerate() {
        let result = ChipRead::try_from(line.as_str());
        assert!(
            result.is_ok(),
            "RAW line {} failed to parse: {:?}",
            i,
            result.err()
        );
        assert_eq!(
            result.unwrap().read_type,
            ReadType::RAW,
            "RAW line {} should be ReadType::RAW",
            i
        );
    }
}

#[test]
fn raw_fixture_line_0_fields() {
    let lines = fixture_lines("raw_reads.txt");
    let read = ChipRead::try_from(lines[0].as_str()).unwrap();

    assert_eq!(read.tag_id, "000000012345");
    assert_eq!(read.read_type, ReadType::RAW);
    assert_eq!(read.timestamp, Timestamp::new(1, 12, 30, 18, 45, 59, 390));
}

#[test]
fn raw_fixture_line_1_fields() {
    let lines = fixture_lines("raw_reads.txt");
    let read = ChipRead::try_from(lines[1].as_str()).unwrap();

    assert_eq!(read.tag_id, "000000012345");
    assert_eq!(read.read_type, ReadType::RAW);
    assert_eq!(read.timestamp, Timestamp::new(1, 12, 30, 18, 46, 0, 0));
}

#[test]
fn raw_fixture_line_2_fields() {
    let lines = fixture_lines("raw_reads.txt");
    let read = ChipRead::try_from(lines[2].as_str()).unwrap();

    assert_eq!(read.tag_id, "00000000AABB");
    assert_eq!(read.read_type, ReadType::RAW);
    // 0x3f = 63 decimal, *10 = 630ms
    assert_eq!(read.timestamp, Timestamp::new(26, 1, 1, 8, 30, 0, 630));
}

// ===========================================================================
// FSLS fixture tests
// ===========================================================================

#[test]
fn fsls_fixture_parses_all_lines() {
    let lines = fixture_lines("fsls_reads.txt");
    assert_eq!(lines.len(), 3, "expected 3 FSLS fixture lines");
    for (i, line) in lines.iter().enumerate() {
        let result = ChipRead::try_from(line.as_str());
        assert!(
            result.is_ok(),
            "FSLS line {} failed to parse: {:?}",
            i,
            result.err()
        );
        assert_eq!(
            result.unwrap().read_type,
            ReadType::FSLS,
            "FSLS line {} should be ReadType::FSLS",
            i
        );
    }
}

#[test]
fn fsls_fixture_line_0_is_fs() {
    let lines = fixture_lines("fsls_reads.txt");
    let read = ChipRead::try_from(lines[0].as_str()).unwrap();

    // Same data as RAW line 0, but with FS suffix
    assert_eq!(read.tag_id, "000000012345");
    assert_eq!(read.read_type, ReadType::FSLS);
    assert_eq!(read.timestamp, Timestamp::new(1, 12, 30, 18, 45, 59, 390));
}

#[test]
fn fsls_fixture_line_1_is_ls() {
    let lines = fixture_lines("fsls_reads.txt");
    let read = ChipRead::try_from(lines[1].as_str()).unwrap();

    assert_eq!(read.tag_id, "000000012345");
    assert_eq!(read.read_type, ReadType::FSLS);
    assert_eq!(read.timestamp, Timestamp::new(1, 12, 30, 18, 46, 0, 0));
}

#[test]
fn tto_enabled_ascii_frame_parses_without_breaking_legacy_fields() {
    let read = ChipRead::try_from(raw_read_with_tto_and_checksum("120080").as_str()).unwrap();

    assert_eq!(read.tag_id, "000000012345");
    assert_eq!(read.timestamp, Timestamp::new(1, 12, 30, 18, 45, 59, 390));
    assert_eq!(read.read_type, ReadType::FSLS);
    assert_eq!(
        read.tto,
        Some(TtoInfo {
            index: 0x12,
            page: 0x00,
            tamper: false,
            first_seen: true,
            last_seen: false,
        })
    );
}

// ===========================================================================
// Date/time range validation tests
// ===========================================================================

/// Build a 36-char chip read string with the given datetime fields and the
/// correct LRC checksum.  All other fields (tag type byte, tag ID, unknown
/// nibble) are taken from the canonical test fixture so existing tests are
/// unaffected.
fn read_with_datetime(year: u8, month: u8, day: u8, hour: u8, min: u8, sec: u8) -> String {
    // body covers positions [2..34]:
    //   [2..4]  = type byte "40"
    //   [4..16] = tag_id "000000012345"
    //   [16..20] = "0a2a" (constant nibble from real fixtures)
    //   [20..22] = year (2-digit decimal)
    //   [22..24] = month
    //   [24..26] = day
    //   [26..28] = hour
    //   [28..30] = minute
    //   [30..32] = second
    //   [32..34] = centisecond hex "27"
    let body = format!(
        "400000000123450a2a{:02}{:02}{:02}{:02}{:02}{:02}27",
        year, month, day, hour, min, sec
    );
    let checksum: u8 = body.bytes().map(|b| b as u32).sum::<u32>() as u8;
    format!("aa{}{:02x}", body, checksum)
}

// --- Invalid ranges ---

#[test]
fn month_zero_is_rejected() {
    let s = read_with_datetime(1, 0, 15, 10, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_err(), "month=0 should be rejected");
    assert_eq!(result.err().unwrap(), "Invalid Chip Read");
}

#[test]
fn month_13_is_rejected() {
    let s = read_with_datetime(1, 13, 15, 10, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_err(), "month=13 should be rejected");
    assert_eq!(result.err().unwrap(), "Invalid Chip Read");
}

#[test]
fn day_zero_is_rejected() {
    let s = read_with_datetime(1, 6, 0, 10, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_err(), "day=0 should be rejected");
    assert_eq!(result.err().unwrap(), "Invalid Chip Read");
}

#[test]
fn day_32_is_rejected() {
    let s = read_with_datetime(1, 6, 32, 10, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_err(), "day=32 should be rejected");
    assert_eq!(result.err().unwrap(), "Invalid Chip Read");
}

#[test]
fn hour_24_is_rejected() {
    let s = read_with_datetime(1, 6, 15, 24, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_err(), "hour=24 should be rejected");
    assert_eq!(result.err().unwrap(), "Invalid Chip Read");
}

#[test]
fn minute_60_is_rejected() {
    let s = read_with_datetime(1, 6, 15, 10, 60, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_err(), "minute=60 should be rejected");
    assert_eq!(result.err().unwrap(), "Invalid Chip Read");
}

#[test]
fn second_60_is_rejected() {
    let s = read_with_datetime(1, 6, 15, 10, 30, 60);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_err(), "second=60 should be rejected");
    assert_eq!(result.err().unwrap(), "Invalid Chip Read");
}

// --- Valid boundary values ---

#[test]
fn month_1_is_accepted() {
    let s = read_with_datetime(1, 1, 15, 10, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_ok(), "month=1 should be accepted");
}

#[test]
fn month_12_is_accepted() {
    let s = read_with_datetime(1, 12, 15, 10, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_ok(), "month=12 should be accepted");
}

#[test]
fn day_1_is_accepted() {
    let s = read_with_datetime(1, 6, 1, 10, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_ok(), "day=1 should be accepted");
}

#[test]
fn day_31_is_accepted() {
    let s = read_with_datetime(1, 1, 31, 10, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_ok(), "day=31 should be accepted");
}

#[test]
fn hour_0_is_accepted() {
    let s = read_with_datetime(1, 6, 15, 0, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_ok(), "hour=0 should be accepted");
}

#[test]
fn hour_23_is_accepted() {
    let s = read_with_datetime(1, 6, 15, 23, 30, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_ok(), "hour=23 should be accepted");
}

#[test]
fn minute_0_is_accepted() {
    let s = read_with_datetime(1, 6, 15, 10, 0, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_ok(), "minute=0 should be accepted");
}

#[test]
fn minute_59_is_accepted() {
    let s = read_with_datetime(1, 6, 15, 10, 59, 45);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_ok(), "minute=59 should be accepted");
}

#[test]
fn second_0_is_accepted() {
    let s = read_with_datetime(1, 6, 15, 10, 30, 0);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_ok(), "second=0 should be accepted");
}

#[test]
fn second_59_is_accepted() {
    let s = read_with_datetime(1, 6, 15, 10, 30, 59);
    let result = ChipRead::try_from(s.as_str());
    assert!(result.is_ok(), "second=59 should be accepted");
}

// ===========================================================================
// Error / edge-case tests (behavior parity with original chip.rs)
// ===========================================================================

#[test]
fn invalid_checksum_is_rejected() {
    // Flip last hex digit of checksum
    let result = ChipRead::try_from("aa400000000123450a2a01123018455927a8");
    assert!(result.is_err());
    assert_eq!(result.err().unwrap(), "Checksum doesn't match");
}

#[test]
fn wrong_length_is_rejected() {
    // 37 chars — neither 36 nor 38
    let result = ChipRead::try_from("aa400000000123450a2a01123018455927a8a");
    assert!(result.is_err());
    assert_eq!(result.err().unwrap(), "Invalid read length");

    // 35 chars
    let result2 = ChipRead::try_from("aa400000000123450a2a01123018455927a");
    assert!(result2.is_err());
    assert_eq!(result2.err().unwrap(), "Invalid read length");
}

#[test]
fn invalid_prefix_is_rejected() {
    let result = ChipRead::try_from("ab400000000123450a2a01123018455927a7");
    assert!(result.is_err());
    assert_eq!(result.err().unwrap(), "Invalid read prefix");
}

#[test]
fn invalid_fsls_suffix_is_rejected() {
    let result = ChipRead::try_from("aa400000000123450a2a01123018455927a7ZZ");
    assert!(result.is_err());
    assert_eq!(result.err().unwrap(), "Invalid read suffix");
}

#[test]
fn lowercase_fsls_suffixes_are_rejected() {
    let fs = ChipRead::try_from("aa400000000123450a2a01123018455927a7fs");
    assert!(fs.is_err());
    assert_eq!(fs.err().unwrap(), "Invalid read suffix");

    let ls = ChipRead::try_from("aa400000000123450a2a01123018455927a7lS");
    assert!(ls.is_err());
    assert_eq!(ls.err().unwrap(), "Invalid read suffix");
}

#[test]
fn empty_input_is_rejected() {
    let result = ChipRead::try_from("   ");
    assert!(result.is_err());
    assert_eq!(result.err().unwrap(), "Empty chip read");
}

#[test]
fn centisecond_boundary_max_valid() {
    // 0x63 = 99 decimal, *10 = 990ms — max valid
    // Build a read with centisec = 63 and correct checksum
    let body = "400000000123450a2a011230184559";
    let centisec = "63";
    let full_body = format!("{}{}", body, centisec);
    let checksum: u8 = full_body.bytes().map(|b| b as u32).sum::<u32>() as u8;
    let read_str = format!("aa{}{:02x}", full_body, checksum);

    let result = ChipRead::try_from(read_str.as_str());
    assert!(result.is_ok());
    assert_eq!(result.unwrap().timestamp.time_string(), "18:45:59.990");
}

#[test]
fn centisecond_over_max_is_rejected() {
    // 0x64 = 100 decimal — over the 0x63 limit
    let body = "400000000123450a2a011230184559";
    let centisec = "64";
    let full_body = format!("{}{}", body, centisec);
    let checksum: u8 = full_body.bytes().map(|b| b as u32).sum::<u32>() as u8;
    let read_str = format!("aa{}{:02x}", full_body, checksum);

    let result = ChipRead::try_from(read_str.as_str());
    assert!(result.is_err());
    assert_eq!(result.err().unwrap(), "Invalid Chip Read");
}

#[test]
fn trailing_whitespace_is_tolerated() {
    // The original parser calls split_whitespace().next(), so trailing
    // spaces / tabs should not affect parsing.
    let result = ChipRead::try_from("aa400000000123450a2a01123018455927a7  \t ");
    assert!(result.is_ok());
    assert_eq!(result.unwrap().tag_id, "000000012345");
}

#[test]
fn display_format_matches_original() {
    let read = ChipRead::try_from("aa400000000123450a2a01123018455927a7").unwrap();
    let display = format!("{}", read);
    assert_eq!(
        display,
        "ID: 000000012345, Type: Streaming, Timestamp: 2001-12-30T18:45:59.390"
    );
}

#[test]
fn read_type_display() {
    assert_eq!(format!("{}", ReadType::RAW), "Streaming");
    assert_eq!(format!("{}", ReadType::FSLS), "FSLS");
}

#[test]
fn read_type_try_from_str() {
    assert_eq!(ReadType::try_from("raw").unwrap(), ReadType::RAW);
    assert_eq!(ReadType::try_from("RAW").unwrap(), ReadType::RAW);
    assert_eq!(ReadType::try_from("fsls").unwrap(), ReadType::FSLS);
    assert_eq!(ReadType::try_from("FSLS").unwrap(), ReadType::FSLS);
    assert!(ReadType::try_from("invalid").is_err());
}

#[test]
fn timestamp_time_string() {
    let ts = Timestamp::new(1, 12, 30, 18, 45, 59, 390);
    assert_eq!(ts.time_string(), "18:45:59.390");
}

#[test]
fn timestamp_display() {
    let ts = Timestamp::new(1, 12, 30, 18, 45, 59, 390);
    assert_eq!(format!("{}", ts), "2001-12-30T18:45:59.390");
}
