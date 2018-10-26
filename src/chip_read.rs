/*
Copyright © 2018  Isaac Wismer

This program is free software: you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation, either version 3 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License
along with this program.  If not, see <http://www.gnu.org/licenses/>.
*/

use std::fmt;
use std::i32;

#[derive(Debug)]
enum ReadType {
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
    tag_id: String,
    timestamp: Timestamp,
    read_type: ReadType,
}

impl ChipRead {
    pub fn new(read: String) -> Result<ChipRead, &'static str> {
        if !(read.len() == 36 || read.len() == 38) {
            return Err("Invalid read length");
        }
        let mut read_type = ReadType::Raw;
        if read.len() == 38
            && (read[37..39] != "FS".to_string() || read[37..39] != "LS".to_string())
        {
            read_type = ReadType::FSLS;
        } else if read.len() == 38 {
            return Err("Invalid read suffix");
        }
        if read[..2] != "aa".to_string() {
            return Err("Invalid read prefix");
        }
        let tag_id = read[4..16].to_string();
        let read_year = match read[20..22].parse::<u16>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(year) => year,
        };
        let read_month = match read[22..24].parse::<u8>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(month) => month,
        };
        let read_day = match read[24..26].parse::<u8>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(day) => day,
        };
        let read_hour = match read[26..28].parse::<u8>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(hour) => hour,
        };
        let read_min = match read[28..30].parse::<u8>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(min) => min,
        };
        let read_sec = match read[30..32].parse::<u8>() {
            Err(_) => return Err("Invalid Chip Read"),
            Ok(sec) => sec,
        };
        let read_millis = match i32::from_str_radix(&read[32..34], 16) {
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

#[derive(Debug)]
struct Timestamp {
    year: u16,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    millis: u16,
}

impl Timestamp {
    fn new(
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        millis: u16,
    ) -> Timestamp {
        Timestamp {
            year: year,
            month: month,
            day: day,
            hour: hour,
            minute: minute,
            second: second,
            millis: millis,
        }
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Customize so only `x` and `y` are denoted.
        write!(
            f,
            "20{:02}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}",
            self.year, self.month, self.day, self.hour, self.minute, self.second, self.millis
        )
    }
}
