use crate::util::io::read_file;
use std::fmt;
use std::i32;

pub enum Gender {
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

impl fmt::Debug for Gender {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let printable = match *self {
            Gender::M => "M".to_string(),
            Gender::F => "F".to_string(),
            Gender::X => "X".to_string(),
        };
        write!(f, "Gender: {}", printable)
    }
}

#[derive(Debug)]
pub struct Participant {
    pub chip_id: Vec<String>,
    pub bib: i32,
    pub first_name: String,
    pub last_name: String,
    pub gender: Gender,
    pub age: Option<i32>,
    pub affiliation: Option<String>,
    pub division: Option<i32>,
}

impl fmt::Display for Participant {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let age_str = match self.age {
            None => "".to_string(),
            Some(age) => format!(", Age: {}", age),
        };
        let affil_str = match self.affiliation {
            None => "".to_string(),
            Some(ref affiliation) => {
                if affiliation == "" {
                    "".to_string()
                } else {
                    format!(", Affiliation: {}", affiliation)
                }
            }
        };
        let division_str = match self.division {
            None => "".to_string(),
            Some(division) => format!(", Division: {}", division),
        };
        write!(
            f,
            "Bib: {}, ID: {:?}, Name: {} {}, Gender: {}{}{}{}",
            self.bib,
            self.chip_id,
            self.first_name,
            self.last_name,
            self.gender,
            age_str,
            affil_str,
            division_str
        )
    }
}

#[allow(dead_code)]
impl Participant {
    pub fn from_ppl_record(record: String) -> Result<Participant, &'static str> {
        let parts = record.split(",").collect::<Vec<&str>>();
        if parts.len() < 3 {
            return Err("Participant Record Error");
        }
        let bib = match parts[0].parse::<i32>() {
            Err(_) => return Err("Participant Record Error"),
            Ok(id) => id,
        };
        let last_name = parts[1];
        let first_name = parts[2];
        let mut affil: Option<String> = None;
        if parts.len() >= 4 {
            affil = Some(parts[3].to_string());
        }
        let mut gender = Gender::X;
        if parts.len() >= 6 {
            gender = match parts[5] {
                "M" | "m" => Gender::M,
                "F" | "f" => Gender::F,
                _ => Gender::X,
            };
        }
        Ok(Participant {
            chip_id: Vec::<String>::new(),
            bib: bib,
            first_name: first_name.to_string(),
            last_name: last_name.to_string(),
            affiliation: affil,
            gender: gender,
            age: None,
            division: None,
        })
    }

    pub fn create_unknown(bib: i32) -> Participant {
        Participant {
            chip_id: Vec::<String>::new(),
            bib: bib,
            first_name: "Unknown".to_string(),
            last_name: "Participant".to_string(),
            affiliation: None,
            gender: Gender::X,
            age: None,
            division: None,
        }
    }
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
