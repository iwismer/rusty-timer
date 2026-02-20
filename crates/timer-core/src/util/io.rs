use crate::models::{ChipBib, Participant};
use encoding::all::WINDOWS_1252;
use encoding::{DecoderTrap, Encoding};
use std::path::Path;

/// Reads a file into a vec of Strings
/// First try reading UTF-8 encoding, if that doesn't work, then read as a
/// WIN1252 encoded file
pub fn read_file(path_str: &str) -> Result<Vec<String>, String> {
    let path = Path::new(path_str);
    let buffer = match std::fs::read(path) {
        Err(desc) => return Err(format!("Couldn't read {}: {}", path.display(), desc)),
        Ok(buf) => buf,
    };
    match std::str::from_utf8(&buffer) {
        Err(_desc) => match WINDOWS_1252.decode(buffer.as_slice(), DecoderTrap::Replace) {
            Err(desc) => Err(format!("Couldn't read {}: {}", path.display(), desc)),
            Ok(s) => Ok(s.to_owned()),
        },
        Ok(s) => Ok(s.to_owned()),
    }
    .map(|s| s.split('\n').map(|s| s.to_owned()).collect())
}

pub fn read_bibchip_file(file_path: &str) -> Result<Vec<ChipBib>, String> {
    let bibs = match read_file(file_path) {
        Err(desc) => {
            return Err(format!("Error reading bibchip file: {}", desc));
        }
        Ok(bibs) => bibs,
    };
    // parse the file and import bib chips into vec
    let mut bib_chip = Vec::new();
    for b in bibs {
        if !b.is_empty() && b.chars().next().unwrap().is_ascii_digit() {
            let parts = b.trim().split(',').collect::<Vec<&str>>();
            if parts.len() < 2 || parts[1].is_empty() {
                eprintln!(
                    "Error reading bibchip file {}. Invalid row: {}",
                    file_path, b
                );
                continue;
            }
            bib_chip.push(ChipBib {
                id: parts[1].to_owned(),
                bib: parts[0].parse::<i32>().unwrap_or_else(|_| {
                    eprintln!(
                        "Error reading bibchip file {}. Invalid bib: {}",
                        file_path, parts[0]
                    );
                    0
                }),
            });
        }
    }
    Ok(bib_chip)
}

pub fn read_participant_file(ppl_path: &str) -> Result<Vec<Participant>, String> {
    let ppl = match read_file(ppl_path) {
        Err(desc) => {
            return Err(format!("Error reading participant file: {}", desc));
        }
        Ok(ppl) => ppl,
    };
    // Read into list of participants and add the chip
    let mut participants = Vec::new();
    for p in ppl {
        // Ignore empty and comment lines
        if !p.is_empty() && !p.starts_with(";") {
            match Participant::from_ppl_record(p.trim()) {
                Err(desc) => println!("Error reading person: {}", desc),
                Ok(person) => {
                    participants.push(person);
                }
            };
        }
    }
    Ok(participants)
}

/// Parse bibchip data from raw bytes (same logic as read_bibchip_file but from memory).
pub fn parse_bibchip_bytes(data: &[u8]) -> Result<Vec<ChipBib>, String> {
    let content = match std::str::from_utf8(data) {
        Ok(s) => s.to_owned(),
        Err(_) => match WINDOWS_1252.decode(data, DecoderTrap::Replace) {
            Ok(s) => s,
            Err(desc) => return Err(format!("failed to decode bibchip bytes: {}", desc)),
        },
    };
    let mut bib_chip = Vec::new();
    let mut invalid_lines: Vec<usize> = Vec::new();
    for (idx, line) in content.split('\n').enumerate() {
        let line = line.trim();
        if !line.is_empty() && line.chars().next().unwrap().is_ascii_digit() {
            let parts: Vec<&str> = line.split(',').collect();
            if parts.len() < 2 || parts[1].is_empty() {
                invalid_lines.push(idx + 1);
                continue;
            }
            let bib = match parts[0].parse::<i32>() {
                Ok(bib) => bib,
                Err(_) => {
                    invalid_lines.push(idx + 1);
                    continue;
                }
            };
            bib_chip.push(ChipBib {
                id: parts[1].trim().to_owned(),
                bib,
            });
        }
    }
    if !invalid_lines.is_empty() {
        return Err(format!(
            "invalid bibchip rows at lines: {}",
            invalid_lines
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<String>>()
                .join(", ")
        ));
    }
    Ok(bib_chip)
}

/// Parse participant data from raw bytes (same logic as read_participant_file but from memory).
pub fn parse_participant_bytes(data: &[u8]) -> Result<Vec<Participant>, String> {
    let content = match std::str::from_utf8(data) {
        Ok(s) => s.to_owned(),
        Err(_) => match WINDOWS_1252.decode(data, DecoderTrap::Replace) {
            Ok(s) => s,
            Err(desc) => return Err(format!("failed to decode participant bytes: {}", desc)),
        },
    };
    let mut participants = Vec::new();
    let mut invalid_lines: Vec<usize> = Vec::new();
    for (idx, line) in content.split('\n').enumerate() {
        let line = line.trim();
        if !line.is_empty() && !line.starts_with(';') {
            match Participant::from_ppl_record(line) {
                Ok(p) => participants.push(p),
                Err(_) => invalid_lines.push(idx + 1),
            }
        }
    }
    if !invalid_lines.is_empty() {
        return Err(format!(
            "invalid participant rows at lines: {}",
            invalid_lines
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<String>>()
                .join(", ")
        ));
    }
    Ok(participants)
}

#[cfg(test)]
mod file_read_tests {
    use super::*;

    #[test]
    fn only_newline() {
        let lines = read_file("test_assets/ppl/empty.ppl");
        assert!(lines.is_ok());
        assert_eq!(lines.unwrap().len(), 1);
    }

    #[test]
    fn single() {
        let lines = read_file("test_assets/ppl/single.ppl");
        assert!(lines.is_ok());
        assert_eq!(lines.unwrap().len(), 2);
    }

    #[test]
    fn multiple() {
        let lines = read_file("test_assets/bibchip/multiple.txt");
        assert!(lines.is_ok());
        assert_eq!(lines.unwrap().len(), 12);
    }

    #[test]
    fn bad_file_path() {
        let lines = read_file("test_assets/ppl/foo.ppl");
        assert!(lines.is_err());
    }

    #[test]
    fn windows_1252() {
        let lines = read_file("test_assets/ppl/windows_1252.ppl");
        assert!(lines.is_ok());
        assert_eq!(lines.unwrap().len(), 2);
    }
}

#[cfg(test)]
mod ppl_tests {
    use super::*;

    #[test]
    fn empty_file() {
        let parts = read_participant_file("test_assets/ppl/empty.ppl");
        assert!(parts.is_ok());
        assert_eq!(parts.unwrap().len(), 0);
    }

    #[test]
    fn only_comments() {
        let parts = read_participant_file("test_assets/ppl/only_comments.ppl");
        assert!(parts.is_ok());
        assert_eq!(parts.unwrap().len(), 0);
    }

    #[test]
    fn invalid_record() {
        let parts = read_participant_file("test_assets/ppl/invalid_record.ppl");
        assert!(parts.is_ok());
        assert_eq!(parts.unwrap().len(), 1);
    }

    #[test]
    fn single() {
        let parts = read_participant_file("test_assets/ppl/single.ppl");
        assert!(parts.is_ok());
        assert_eq!(parts.unwrap().len(), 1);
    }

    #[test]
    fn bad_file_path() {
        let parts = read_participant_file("test_assets/ppl/foo.ppl");
        assert!(parts.is_err());
    }

    #[test]
    fn windows_1252() {
        let parts = read_participant_file("test_assets/ppl/windows_1252.ppl");
        assert!(parts.is_ok());
        assert_eq!(parts.unwrap().len(), 1);
    }
}

#[cfg(test)]
mod bibchip_tests {
    use super::*;

    #[test]
    fn empty_file() {
        let bibs = read_bibchip_file("test_assets/bibchip/empty.txt");
        assert!(bibs.is_ok());
        assert_eq!(bibs.unwrap().len(), 0);
    }

    #[test]
    fn invalid_record() {
        let bibs = read_bibchip_file("test_assets/bibchip/invalid_record.txt");
        assert!(bibs.is_ok());
        assert_eq!(bibs.unwrap().len(), 10);
    }

    #[test]
    fn single() {
        let bibs = read_bibchip_file("test_assets/bibchip/single.txt");
        assert!(bibs.is_ok());
        assert_eq!(bibs.unwrap().len(), 1);
    }

    #[test]
    fn multiple() {
        let bibs = read_bibchip_file("test_assets/bibchip/multiple.txt");
        assert!(bibs.is_ok());
        assert_eq!(bibs.unwrap().len(), 10);
    }

    #[test]
    fn bad_file_path() {
        let bibs = read_bibchip_file("test_assets/bibchip/foo.txt");
        assert!(bibs.is_err());
    }

    #[test]
    fn malformed_numeric_row_missing_chip_is_skipped() {
        let bibs = read_bibchip_file("test_assets/bibchip/malformed_missing_fields.txt");
        assert!(bibs.is_ok());
        let bibs = bibs.unwrap();
        assert_eq!(bibs.len(), 1);
        assert_eq!(bibs[0].bib, 4401);
        assert_eq!(bibs[0].id, "05800374ea00");
    }
}

#[cfg(test)]
mod bytes_parser_tests {
    use super::*;

    #[test]
    fn parse_bibchip_bytes_basic() {
        let data = b"BIB,CHIP\n1,058003700001\n2,058003700002\n";
        let result = parse_bibchip_bytes(data).expect("valid bibchip data");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].bib, 1);
        assert_eq!(result[0].id, "058003700001");
        assert_eq!(result[1].bib, 2);
    }

    #[test]
    fn parse_bibchip_bytes_skips_header_and_empty() {
        let data = b"BIB,CHIP\n\n1,chip1\n";
        let result = parse_bibchip_bytes(data).expect("valid bibchip data");
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn parse_bibchip_bytes_empty() {
        let result = parse_bibchip_bytes(b"").expect("empty data should parse");
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn parse_bibchip_bytes_invalid_bib_is_rejected() {
        let data = b"BIB,CHIP\n1x,058003700001\n";
        let result = parse_bibchip_bytes(data);
        assert!(result.is_err(), "invalid bib rows should be rejected");
    }

    #[test]
    fn parse_participant_bytes_basic() {
        let data = b"1,Smith,John,Team A,,M\n2,Doe,Jane,Team B,,F\n";
        let result = parse_participant_bytes(data).expect("valid participant data");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].bib, 1);
        assert_eq!(result[0].first_name, "John");
        assert_eq!(result[0].last_name, "Smith");
    }

    #[test]
    fn parse_participant_bytes_skips_comments() {
        let data = b";This is a comment\n1,Smith,John,,,M\n";
        let result = parse_participant_bytes(data).expect("valid participant data");
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn parse_participant_bytes_empty() {
        let result = parse_participant_bytes(b"").expect("empty data should parse");
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn parse_participant_bytes_invalid_row_is_rejected() {
        let data = b"1\n";
        let result = parse_participant_bytes(data);
        assert!(
            result.is_err(),
            "invalid participant rows should be rejected"
        );
    }
}
