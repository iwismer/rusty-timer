use encoding::all::WINDOWS_1252;
use encoding::{DecoderTrap, Encoding};
use std::path::Path;
use crate::models::{ChipBib, Participant};

/// Reads a file into a vec of Strings
/// First try reading UTF-8 encoding, if that doesn't work, then read as a
/// WIN1252 encoded file
pub fn read_file(path_str: &String) -> Result<Vec<String>, String> {
    let path = Path::new(path_str);
    let buffer = match std::fs::read(path) {
        Err(desc) => {
            return Err(format!(
                "Couldn't read {}: {}",
                path.display(),
                desc.to_string()
            ))
        }
        Ok(buf) => buf,
    };
    match std::str::from_utf8(&buffer) {
        Err(_desc) => match WINDOWS_1252.decode(buffer.as_slice(), DecoderTrap::Replace) {
            Err(desc) => Err(format!("Couldn't read {}: {}", path.display(), desc)),
            Ok(s) => Ok(s.to_string()),
        },
        Ok(s) => Ok(s.to_string()),
    }
    .map(|s| s.split('\n').map(|s| s.to_string()).collect())
}

pub fn read_bibchip_file(file_path: String) -> Result<Vec<ChipBib>, String> {
    let bibs = match read_file(&file_path) {
        Err(desc) => {
            return Err(format!("Error reading bibchip file: {}", desc));
        }
        Ok(bibs) => bibs,
    };
    // parse the file and import bib chips into vec
    let mut bib_chip = Vec::new();
    for b in bibs {
        if b != "" && b.chars().next().unwrap().is_digit(10) {
            let parts = b.trim().split(",").collect::<Vec<&str>>();
            bib_chip.push(ChipBib {
                id: parts[1].to_string(),
                bib: parts[0].parse::<i32>().unwrap_or_else(|_| {
                    println!("Error reading bib file. Invalid bib: {}", parts[0]);
                    0
                }),
            });
        }
    }
    Ok(bib_chip)
}

pub fn read_participant_file(ppl_path: String) -> Result<Vec<Participant>, String> {
    let ppl = match read_file(&ppl_path) {
        Err(desc) => {
            return Err(format!("Error reading participant file: {}", desc));
        }
        Ok(ppl) => ppl,
    };
    // Read into list of participants and add the chip
    let mut participants = Vec::new();
    for p in ppl {
        // Ignore empty and comment lines
        if p != "" && !p.starts_with(";") {
            match Participant::from_ppl_record(p.trim().to_string()) {
                Err(desc) => println!("Error reading person: {}", desc),
                Ok(person) => {
                    participants.push(person);
                }
            };
        }
    }
    Ok(participants)
}


#[cfg(test)]
mod file_read_tests {
    use super::*;

    #[test]
    fn only_newline() {
        let lines = read_file(&"test_assets/ppl/empty.ppl".to_string());
        assert!(lines.is_ok());
        assert_eq!(lines.unwrap().len(), 1);
    }

    #[test]
    fn single() {
        let lines = read_file(&"test_assets/ppl/single.ppl".to_string());
        assert!(lines.is_ok());
        assert_eq!(lines.unwrap().len(), 2);
    }

    #[test]
    fn multiple() {
        let lines = read_file(&"test_assets/bibchip/multiple.txt".to_string());
        assert!(lines.is_ok());
        assert_eq!(lines.unwrap().len(), 12);
    }

    #[test]
    fn bad_file_path() {
        let lines = read_file(&"test_assets/ppl/foo.ppl".to_string());
        assert!(lines.is_err());
    }

    #[test]
    fn windows_1252() {
        let lines = read_file(&"test_assets/ppl/windows_1252.ppl".to_string());
        assert!(lines.is_ok());
        assert_eq!(lines.unwrap().len(), 2);
    }
}

#[cfg(test)]
mod ppl_tests {
    use super::*;

    #[test]
    fn empty_file() {
        let parts = read_participant_file("test_assets/ppl/empty.ppl".to_string());
        assert!(parts.is_ok());
        assert_eq!(parts.unwrap().len(), 0);
    }

    #[test]
    fn only_comments() {
        let parts = read_participant_file("test_assets/ppl/only_comments.ppl".to_string());
        assert!(parts.is_ok());
        assert_eq!(parts.unwrap().len(), 0);
    }

    #[test]
    fn invalid_record() {
        let parts = read_participant_file("test_assets/ppl/invalid_record.ppl".to_string());
        assert!(parts.is_ok());
        assert_eq!(parts.unwrap().len(), 1);
    }

    #[test]
    fn single() {
        let parts = read_participant_file("test_assets/ppl/single.ppl".to_string());
        assert!(parts.is_ok());
        assert_eq!(parts.unwrap().len(), 1);
    }

    #[test]
    fn bad_file_path() {
        let parts = read_participant_file("test_assets/ppl/foo.ppl".to_string());
        assert!(parts.is_err());
    }

    #[test]
    fn windows_1252() {
        let parts = read_participant_file("test_assets/ppl/windows_1252.ppl".to_string());
        assert!(parts.is_ok());
        assert_eq!(parts.unwrap().len(), 1);
    }
}

#[cfg(test)]
mod bibchip_tests {
    use super::*;

    #[test]
    fn empty_file() {
        let bibs = read_bibchip_file("test_assets/bibchip/empty.txt".to_string());
        assert!(bibs.is_ok());
        assert_eq!(bibs.unwrap().len(), 0);
    }

    #[test]
    fn invalid_record() {
        let bibs = read_bibchip_file("test_assets/bibchip/invalid_record.txt".to_string());
        assert!(bibs.is_ok());
        assert_eq!(bibs.unwrap().len(), 10);
    }

    #[test]
    fn single() {
        let bibs = read_bibchip_file("test_assets/bibchip/single.txt".to_string());
        assert!(bibs.is_ok());
        assert_eq!(bibs.unwrap().len(), 1);
    }

    #[test]
    fn multiple() {
        let bibs = read_bibchip_file("test_assets/bibchip/multiple.txt".to_string());
        assert!(bibs.is_ok());
        assert_eq!(bibs.unwrap().len(), 10);
    }

    #[test]
    fn bad_file_path() {
        let bibs = read_bibchip_file("test_assets/bibchip/foo.txt".to_string());
        assert!(bibs.is_err());
    }
}
