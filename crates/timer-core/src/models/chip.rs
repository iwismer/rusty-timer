// Re-export core IPICO parsing types from ipico-core.
// The canonical implementation lives in crates/ipico-core/src/read.rs.
pub use ipico_core::read::{ChipRead, ReadType};

/// A struct for mapping a chip to a bib number
#[derive(Debug, Eq, Ord, PartialOrd, PartialEq, Clone)]
pub struct ChipBib {
    pub id: String,
    pub bib: i32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Timestamp;
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
