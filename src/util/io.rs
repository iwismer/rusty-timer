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
                "couldn't read {}: {}",
                path.display(),
                desc.to_string()
            ))
        }
        Ok(buf) => buf,
    };
    match std::str::from_utf8(&buffer) {
        Err(_desc) => match WINDOWS_1252.decode(buffer.as_slice(), DecoderTrap::Replace) {
            Err(desc) => Err(format!("couldn't read {}: {}", path.display(), desc)),
            Ok(s) => Ok(s.to_string()),
        },
        Ok(s) => Ok(s.to_string()),
    }
    .map(|s| s.split('\n').map(|s| s.to_string()).collect())
}

pub fn read_bibchip_file(file_path: String) -> Vec<ChipBib> {
    let bibs = match read_file(&file_path) {
        Err(desc) => {
            println!("Error reading bibchip file {}", desc);
            Vec::new()
        }
        Ok(bibs) => bibs,
    };
    // parse the file and import bib chips into hashmap
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
    bib_chip
}

pub fn read_participant_file(ppl_path: String) -> Vec<Participant> {
    let ppl = match read_file(&ppl_path) {
        Err(desc) => {
            println!("Error reading participant file {}", desc);
            Vec::new()
        }
        Ok(ppl) => ppl,
    };
    // Read into list of participants and add the chip
    let mut participants = Vec::new();
    for p in ppl {
        // Ignore empty and comment lines
        if p != "" && !p.starts_with(";") {
            match Participant::from_ppl_record(p.trim().to_string()) {
                Err(desc) => println!("Error reading person {}", desc),
                Ok(person) => {
                    participants.push(person);
                }
            };
        }
    }
    participants
}
