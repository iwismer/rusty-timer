//! IPICO chip read parsing.
//!
//! This module contains the core types and parsing logic for IPICO timing
//! system chip reads. It is extracted from the original `src/models/chip.rs`
//! and `src/models/timestamp.rs` in the rusty-timer crate for shared use
//! across the remote forwarding suite services.
//!
//! # UTF-8 requirement
//!
//! The parser accepts `&str`, which guarantees valid UTF-8 at the type level.
//! Callers must reject invalid UTF-8 before invoking the parser — the design
//! intentionally does **not** silently rewrite bad bytes.

use std::convert::TryFrom;
use std::fmt;

// ---------------------------------------------------------------------------
// Timestamp
// ---------------------------------------------------------------------------

/// A simple timestamp with year (two-digit), month, day, hour, minute,
/// second, and millisecond fields.
#[derive(Debug, Eq, Ord, PartialOrd, PartialEq, Copy, Clone)]
pub struct Timestamp {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    millis: u16,
}

impl Timestamp {
    pub fn new(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        millis: u16,
    ) -> Timestamp {
        Timestamp {
            year,
            month,
            day,
            hour,
            minute,
            second,
            millis,
        }
    }

    pub fn time_string(&self) -> String {
        format!(
            "{:02}:{:02}:{:02}.{:03}",
            self.hour, self.minute, self.second, self.millis
        )
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "20{:02}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.millis
        )
    }
}

// ---------------------------------------------------------------------------
// ReadType
// ---------------------------------------------------------------------------

/// Define a read as either raw, or first-seen/last-seen.
#[derive(Debug, Eq, Ord, PartialOrd, PartialEq, Copy, Clone)]
#[allow(clippy::upper_case_acronyms)]
pub enum ReadType {
    RAW = 38,
    FSLS = 40,
}

impl fmt::Display for ReadType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            ReadType::RAW => write!(f, "Streaming"),
            ReadType::FSLS => write!(f, "FSLS"),
        }
    }
}

impl TryFrom<&str> for ReadType {
    type Error = &'static str;

    fn try_from(type_str: &str) -> Result<Self, Self::Error> {
        match type_str.to_lowercase().as_str() {
            "raw" => Ok(ReadType::RAW),
            "fsls" => Ok(ReadType::FSLS),
            _ => Err("Invalid read type"),
        }
    }
}

// ---------------------------------------------------------------------------
// ChipRead
// ---------------------------------------------------------------------------

/// A parsed IPICO chip read containing the tag ID, timestamp, and read type.
#[derive(Debug, Eq, Ord, PartialOrd, PartialEq, Clone)]
pub struct ChipRead {
    pub tag_id: String,
    pub timestamp: Timestamp,
    pub read_type: ReadType,
}

#[allow(dead_code)]
impl ChipRead {
    pub fn cmp(a: ChipRead, b: ChipRead) -> std::cmp::Ordering {
        a.timestamp.cmp(&b.timestamp)
    }

    pub fn time_string(&self) -> String {
        self.timestamp.time_string()
    }
}

impl TryFrom<&str> for ChipRead {
    type Error = &'static str;

    fn try_from(read_str: &str) -> Result<Self, Self::Error> {
        let chip_read = read_str
            .split_whitespace()
            .next()
            .ok_or("Empty chip read")?;
        if !(chip_read.len() == 36 || chip_read.len() == 38) {
            return Err("Invalid read length");
        }
        let checksum = chip_read[2..34].bytes().map(|b| b as u32).sum::<u32>() as u8;
        if format!("{:02x}", checksum) != chip_read[34..36] {
            return Err("Checksum doesn't match");
        }
        let read_type = if chip_read.len() == 38 {
            match &chip_read[36..38] {
                "FS" | "LS" => ReadType::FSLS,
                _ => return Err("Invalid read suffix"),
            }
        } else {
            ReadType::RAW
        };
        if &chip_read[..2] != "aa" {
            return Err("Invalid read prefix");
        }
        let tag_id = chip_read[4..16].to_owned();
        let read_year = match chip_read[20..22].parse::<u16>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(year) => year,
        };
        let read_month = match chip_read[22..24].parse::<u8>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(month) => month,
        };
        let read_day = match chip_read[24..26].parse::<u8>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(day) => day,
        };
        let read_hour = match chip_read[26..28].parse::<u8>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(hour) => hour,
        };
        let read_min = match chip_read[28..30].parse::<u8>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(min) => min,
        };
        let read_sec = match chip_read[30..32].parse::<u8>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(sec) => sec,
        };
        let read_millis = match i32::from_str_radix(&chip_read[32..34], 16) {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(millis) => {
                if millis > 0x63 {
                    return Err("Invalid Chip Read");
                }
                (millis * 10) as u16
            }
        };
        let read_time: Timestamp = Timestamp::new(
            read_year,
            read_month,
            read_day,
            read_hour,
            read_min,
            read_sec,
            read_millis,
        );
        Ok(ChipRead {
            tag_id,
            timestamp: read_time,
            read_type,
        })
    }
}

impl fmt::Display for ChipRead {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "ID: {}, Type: {}, Timestamp: {}",
            self.tag_id, self.read_type, self.timestamp
        )
    }
}

// ---------------------------------------------------------------------------
// Unit tests (ported from original chip.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::TryFrom;

    fn raw_read_with_checksum(centisecond_hex: &str) -> String {
        let mut read = format!("aa400000000123450a2a011230184559{}", centisecond_hex);
        let checksum = read[2..34].bytes().map(|b| b as u32).sum::<u32>() as u8;
        read.push_str(&format!("{:02x}", checksum));
        read
    }

    #[test]
    fn simple_chip() {
        let read = ChipRead::try_from("aa400000000123450a2a01123018455927a7");
        assert!(read.is_ok());
        assert_eq!(
            read.unwrap(),
            ChipRead {
                tag_id: "000000012345".to_owned(),
                timestamp: Timestamp::new(1, 12, 30, 18, 45, 59, 390),
                read_type: ReadType::RAW
            }
        );
    }

    #[test]
    fn invalid_checksum() {
        let read = ChipRead::try_from("aa400000000123450a2a01123018455927a8");
        assert!(read.is_err());
        assert_eq!(read.err().unwrap(), "Checksum doesn't match");

        let read2 = ChipRead::try_from("aa400000000123450a2a01123018455927ff");
        assert!(read2.is_err());
        assert_eq!(read2.err().unwrap(), "Checksum doesn't match");
    }

    #[test]
    fn wrong_length() {
        let read = ChipRead::try_from("aa400000000123450a2a01123018455927a8a");
        assert!(read.is_err());
        assert_eq!(read.err().unwrap(), "Invalid read length");

        let read2 = ChipRead::try_from("aa400000000123450a2a01123018455927a");
        assert!(read2.is_err());
        assert_eq!(read2.err().unwrap(), "Invalid read length");
    }

    #[test]
    fn invalid_header() {
        let read = ChipRead::try_from("ab400000000123450a2a01123018455927a7");
        assert!(read.is_err());
        assert_eq!(read.err().unwrap(), "Invalid read prefix");
    }

    #[test]
    fn fsls_suffixes() {
        let read_fs = ChipRead::try_from("aa400000000123450a2a01123018455927a7FS");
        assert!(read_fs.is_ok());
        assert_eq!(read_fs.unwrap().read_type, ReadType::FSLS);

        let read_ls = ChipRead::try_from("aa400000000123450a2a01123018455927a7LS");
        assert!(read_ls.is_ok());
        assert_eq!(read_ls.unwrap().read_type, ReadType::FSLS);
    }

    #[test]
    fn invalid_fsls_suffix() {
        let read = ChipRead::try_from("aa400000000123450a2a01123018455927a7ZZ");
        assert!(read.is_err());
        assert_eq!(read.err().unwrap(), "Invalid read suffix");
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
    fn centisecond_bounds() {
        let low_read = raw_read_with_checksum("00");
        let low = ChipRead::try_from(low_read.as_str());
        assert!(low.is_ok());
        assert_eq!(low.unwrap().time_string(), "18:45:59.000");

        let high_read = raw_read_with_checksum("63");
        let high = ChipRead::try_from(high_read.as_str());
        assert!(high.is_ok());
        assert_eq!(high.unwrap().time_string(), "18:45:59.990");
    }

    #[test]
    fn invalid_centisecond_value_is_rejected() {
        let invalid_read = raw_read_with_checksum("64");
        let invalid = ChipRead::try_from(invalid_read.as_str());
        assert!(invalid.is_err());
        assert_eq!(invalid.err().unwrap(), "Invalid Chip Read");
    }

    #[test]
    fn empty_read_returns_error() {
        let result = ChipRead::try_from("   ");
        assert!(result.is_err());
        assert_eq!(result.err().unwrap(), "Empty chip read");
    }
}
