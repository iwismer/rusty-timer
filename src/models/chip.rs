use super::Timestamp;
use std::convert::TryFrom;
use std::fmt;
use std::i32;

/// A struct for mapping a chip to a bib number
#[derive(Debug, Eq, Ord, PartialOrd, PartialEq, Clone)]
pub struct ChipBib {
    pub id: String,
    pub bib: i32,
}

/// Define a read as either raw, or first-seen/last-seen
#[derive(Debug, Eq, Ord, PartialOrd, PartialEq, Copy, Clone)]
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
        let chip_read = read_str.trim().split_whitespace().next().unwrap();
        if !(chip_read.len() == 36 || chip_read.len() == 38) {
            return Err("Invalid read length");
        }
        let checksum = chip_read[2..34].bytes().map(|b| b as u32).sum::<u32>() as u8;
        if format!("{:02x}", checksum) != chip_read[34..36] {
            return Err("Checksum doesn't match");
        }
        let mut read_type = ReadType::RAW;
        if chip_read.len() == 38 && (&chip_read[37..] != "FS" || &chip_read[37..] != "LS") {
            read_type = ReadType::FSLS;
        } else if chip_read.len() == 38 {
            return Err("Invalid read suffix");
        }
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
            Ok(millis) => (millis * 10) as u16,
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
            tag_id: tag_id,
            timestamp: read_time,
            read_type: read_type,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::TryFrom;

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
}
