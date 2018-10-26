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

use std::i32;

enum Gender {
    M,
    F,
    X,
}

impl fmt::Display for Gender {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let printable = match *self {
            Gender::M => "M".to_string(),
            Gender::F => "F".to_string(),
            Gender::X => "X".to_string(),
        };
        write!(f, "{}", printable)
    }
}

#[derive(Debug)]
pub struct Participant {
    chip_id: Vec<String>,
    bib: i32,
    first_name: String,
    last_name: String,
    gender: Gender,
    age: Option<i32>,
    affiliation: Option<String>,
    division: Option<i32>,
}

impl fmt::Display for Participant {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let age_str = match self.age {
            None() => "".to_string(),
            Some(age) => format!("Age: {}, ", age),
        };
        let affil_str = match self.affiliation {
            None() => "".to_string(),
            Some(affiliation) => format!("Affiliation: {}, ", affiliation),
        };
        write!(
            f,
            "Bib: {}, ID: {}, Name: {} {}, Gender: {}, {}{}",
            self.bib,
            self.chip_id,
            self.first_name,
            self.last_name,
            self.gender,
            age_str,
            affil_str,
            self.division
        )
    }
}

impl Participant {
    fn from_ppl_record(record: String) -> Result(Participant) {
        let parts = record.split(",");
        if parts.len() < 3 {
            Err("Participant Record Error")
        }
        let bib = match parts[0].parse::<i32>() {
            Err(_) => return Err("Participant Record Error"),
            Ok(id) => id,
        };
        let last_name = parts[1];
        let first_name = parts[2];
        let mut affiliation: Option<String> = None();
        if parts.len() >= 4 {
            affiliation = Some(parts[3]);
        }
        Participant {chip_id: Vec<String>::new(), bib: bib, first_name: first_name, last_name: last_name, affiliation: affiliation, gender: Gender::M};
    }
}
