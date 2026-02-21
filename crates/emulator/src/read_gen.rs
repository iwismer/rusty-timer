use chrono::{Datelike, Timelike};
use ipico_core::read::ReadType;

pub fn generate_read_for_time(read_type: ReadType, now: chrono::DateTime<chrono::Local>) -> String {
    let centiseconds = (now.nanosecond() / 10_000_000) as u8;
    let read = format!(
        "aa00{}{:02}{:02}{:02}{:02}{:02}{:02}{:02x}",
        "05800319aeeb0001",
        now.year() % 100,
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        centiseconds
    );
    let checksum = read[2..34].bytes().map(|b| b as u32).sum::<u32>() as u8;
    match read_type {
        ReadType::RAW => format!("{read}{checksum:02x}"),
        ReadType::FSLS => format!("{read}{checksum:02x}LS"),
    }
}

pub fn generate_read(read_type: ReadType) -> String {
    generate_read_for_time(read_type, chrono::Local::now())
}

/// Replace the timestamp in a chip read with the given time and recompute the
/// checksum. If the read doesn't look like a valid IPICO read (wrong length or
/// prefix), it is returned as-is.
pub fn restamp_read_for_time(read: &str, now: chrono::DateTime<chrono::Local>) -> String {
    let trimmed = read.trim();
    if (trimmed.len() != 36 && trimmed.len() != 38) || !trimmed.starts_with("aa") {
        return trimmed.to_owned();
    }
    let centiseconds = (now.nanosecond() / 10_000_000) as u8;
    let new_timestamp = format!(
        "{:02}{:02}{:02}{:02}{:02}{:02}{:02x}",
        now.year() % 100,
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        centiseconds
    );
    let mut new_read = String::with_capacity(trimmed.len());
    new_read.push_str(&trimmed[..20]);
    new_read.push_str(&new_timestamp);
    let checksum = new_read[2..34].bytes().map(|b| b as u32).sum::<u32>() as u8;
    new_read.push_str(&format!("{checksum:02x}"));
    if trimmed.len() == 38 {
        new_read.push_str(&trimmed[36..38]);
    }
    new_read
}

pub fn restamp_read(read: &str) -> String {
    restamp_read_for_time(read, chrono::Local::now())
}

/// Generate an IPICO read string from a numeric chip ID and timestamp components.
///
/// Converts `chip_id` to a 12-char hex tag ID (zero-padded). Uses reader_id `0x00`
/// and I/Q counters `0x0000`. The result passes `ChipRead::try_from()`.
///
/// `centiseconds` must be 0..=99.
#[allow(clippy::too_many_arguments)]
pub fn generate_read_for_chip(
    chip_id: u64,
    read_type: ReadType,
    year: u8,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    centiseconds: u8,
) -> String {
    let read = format!(
        "aa00{:012x}0000{:02}{:02}{:02}{:02}{:02}{:02}{:02x}",
        chip_id, year, month, day, hour, minute, second, centiseconds
    );
    let checksum = read[2..34].bytes().map(|b| b as u32).sum::<u32>() as u8;
    match read_type {
        ReadType::RAW => format!("{read}{checksum:02x}"),
        ReadType::FSLS => format!("{read}{checksum:02x}LS"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use ipico_core::read::ChipRead;
    use std::convert::TryFrom;

    #[test]
    fn generated_raw_reads_parse() {
        let read = generate_read(ReadType::RAW);
        let parsed = ChipRead::try_from(read.as_str());
        assert!(parsed.is_ok());
    }

    #[test]
    fn generated_fsls_reads_parse() {
        let read = generate_read(ReadType::FSLS);
        let parsed = ChipRead::try_from(read.as_str());
        assert!(parsed.is_ok());
    }

    #[test]
    fn generated_read_shapes_are_stable() {
        let raw = generate_read(ReadType::RAW);
        assert_eq!(raw.len(), 36);
        assert!(raw.starts_with("aa"));

        let fsls = generate_read(ReadType::FSLS);
        assert_eq!(fsls.len(), 38);
        assert!(fsls.ends_with("LS"));
    }

    #[test]
    fn generated_read_encodes_centiseconds_as_hex() {
        let now = chrono::Local.with_ymd_and_hms(2025, 1, 2, 3, 4, 5).unwrap()
            + chrono::TimeDelta::milliseconds(990);
        let read = generate_read_for_time(ReadType::RAW, now);
        assert_eq!(&read[32..34], "63");

        let parsed = ChipRead::try_from(read.as_str()).unwrap();
        assert_eq!(parsed.time_string(), "03:04:05.990");
    }

    #[test]
    fn restamp_preserves_tag_and_updates_timestamp() {
        let original = "aa400000000123450a2a01123018455927a7";
        let now = chrono::Local
            .with_ymd_and_hms(2025, 6, 15, 10, 30, 45)
            .unwrap();
        let restamped = restamp_read_for_time(original, now);
        assert_eq!(&restamped[4..20], "0000000123450a2a");
        let parsed = ChipRead::try_from(restamped.as_str()).unwrap();
        assert_eq!(parsed.time_string(), "10:30:45.000");
    }

    #[test]
    fn restamp_preserves_fsls_suffix() {
        let original = "aa400000000123450a2a01123018455927a7LS";
        let now = chrono::Local
            .with_ymd_and_hms(2025, 6, 15, 10, 30, 45)
            .unwrap();
        let restamped = restamp_read_for_time(original, now);
        assert!(restamped.ends_with("LS"));
        assert_eq!(restamped.len(), 38);
        assert!(ChipRead::try_from(restamped.as_str()).is_ok());
    }

    #[test]
    fn restamp_returns_invalid_reads_unchanged() {
        let now = chrono::Local
            .with_ymd_and_hms(2025, 6, 15, 10, 30, 45)
            .unwrap();
        assert_eq!(restamp_read_for_time("not a read", now), "not a read");
        assert_eq!(restamp_read_for_time("", now), "");
    }

    #[test]
    fn generate_read_for_chip_produces_valid_raw_read() {
        let read = generate_read_for_chip(1000, ReadType::RAW, 26, 1, 1, 0, 0, 0, 0);
        let parsed = ChipRead::try_from(read.as_str());
        assert!(parsed.is_ok(), "must produce parseable IPICO read: {read}");
        assert_eq!(read.len(), 36);
    }

    #[test]
    fn generate_read_for_chip_produces_valid_fsls_read() {
        let read = generate_read_for_chip(1000, ReadType::FSLS, 26, 1, 1, 0, 0, 0, 0);
        let parsed = ChipRead::try_from(read.as_str());
        assert!(parsed.is_ok(), "must produce parseable IPICO read: {read}");
        assert_eq!(read.len(), 38);
        assert!(read.ends_with("LS"));
    }

    #[test]
    fn generate_read_for_chip_encodes_chip_id_as_hex_tag() {
        let read = generate_read_for_chip(1000, ReadType::RAW, 26, 1, 1, 0, 0, 0, 0);
        // chip_id 1000 = 0x3e8, zero-padded to 12 chars = "0000000003e8"
        assert_eq!(&read[4..16], "0000000003e8");
    }

    #[test]
    fn generate_read_for_chip_encodes_timestamp() {
        let read = generate_read_for_chip(1, ReadType::RAW, 26, 6, 15, 10, 30, 45, 50);
        let parsed = ChipRead::try_from(read.as_str()).unwrap();
        assert_eq!(parsed.time_string(), "10:30:45.500");
    }

    #[test]
    fn generate_read_for_chip_different_ids_produce_different_tags() {
        let read_a = generate_read_for_chip(1000, ReadType::RAW, 26, 1, 1, 0, 0, 0, 0);
        let read_b = generate_read_for_chip(2000, ReadType::RAW, 26, 1, 1, 0, 0, 0, 0);
        assert_ne!(&read_a[4..16], &read_b[4..16]);
    }
}
