use super::Timestamp;
use crate::util::io::read_file;
use std::fmt;
use std::i32;

pub struct ChipBib {
    pub id: String,
    pub bib: i32,
}

#[derive(Debug)]
pub enum ReadType {
    Raw,
    FSLS,
}

impl fmt::Display for ReadType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let printable = match *self {
            ReadType::Raw => "Streaming".to_string(),
            ReadType::FSLS => "FSLS".to_string(),
        };
        write!(f, "{}", printable)
    }
}

#[derive(Debug)]
pub struct ChipRead {
    pub tag_id: String,
    pub timestamp: Timestamp,
    pub read_type: ReadType,
}

#[allow(dead_code)]
impl ChipRead {
    pub fn new(read_str: String) -> Result<ChipRead, &'static str> {
        let mut chip_read = read_str.trim().to_string();
        chip_read = chip_read.split_whitespace().next().unwrap().to_string();
        if !(chip_read.len() == 36 || chip_read.len() == 38) {
            return Err("Invalid read length");
        }
        let mut read_type = ReadType::Raw;
        if chip_read.len() == 38
            && (chip_read[37..39] != "FS".to_string() || chip_read[37..39] != "LS".to_string())
        {
            read_type = ReadType::FSLS;
        } else if chip_read.len() == 38 {
            return Err("Invalid read suffix");
        }
        if chip_read[..2] != "aa".to_string() {
            return Err("Invalid read prefix");
        }
        let tag_id = chip_read[4..16].to_string();
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

    pub fn cmp(a: ChipRead, b: ChipRead) -> std::cmp::Ordering {
        a.timestamp.cmp(&b.timestamp)
    }

    pub fn time_string(&self) -> String {
        self.timestamp.time_string()
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
