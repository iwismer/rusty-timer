use std::fmt;

#[derive(Eq, Ord, PartialOrd, PartialEq, Clone)]
pub enum Gender {
    M,
    F,
    X,
}

impl fmt::Display for Gender {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Gender::M => write!(f, "M"),
            Gender::F => write!(f, "F"),
            Gender::X => write!(f, "X"),
        }
    }
}

impl fmt::Debug for Gender {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Gender::M => write!(f, "Gender: M"),
            Gender::F => write!(f, "Gender: F"),
            Gender::X => write!(f, "Gender: X"),
        }
    }
}

/// A single race participant
#[derive(Debug, Eq, Ord, PartialOrd, PartialEq, Clone)]
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
            None => "".to_owned(),
            Some(age) => format!(", Age: {}", age),
        };
        let affil_str = match self.affiliation {
            None => "".to_owned(),
            Some(ref affiliation) => {
                if affiliation.is_empty() {
                    "".to_owned()
                } else {
                    format!(", Affiliation: {}", affiliation)
                }
            }
        };
        let division_str = match self.division {
            None => "".to_owned(),
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
    pub fn from_ppl_record(record: &str) -> Result<Participant, &'static str> {
        let parts = record.split(",").collect::<Vec<&str>>();
        if parts.len() < 3 {
            return Err("Participant Record Error");
        }
        let bib = match parts[0].parse::<i32>() {
            Err(_) => return Err("Participant Record Error"),
            Ok(id) => id,
        };
        let last_name = parts[1].to_owned();
        let first_name = parts[2].to_owned();
        let mut affil: Option<String> = None;
        if parts.len() >= 4 {
            affil = Some(parts[3].to_owned());
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
            bib,
            first_name,
            last_name,
            affiliation: affil,
            gender,
            age: None,
            division: None,
        })
    }

    pub fn create_unknown(bib: i32) -> Participant {
        Participant {
            chip_id: Vec::<String>::new(),
            bib,
            first_name: "Unknown".to_owned(),
            last_name: "Participant".to_owned(),
            affiliation: None,
            gender: Gender::X,
            age: None,
            division: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_ppl() {
        let part = Participant::from_ppl_record("0,Smith,John,Team Smith,,M");
        assert!(part.is_ok());
        assert_eq!(
            part.unwrap(),
            Participant {
                chip_id: Vec::<String>::new(),
                bib: 0,
                first_name: "John".to_owned(),
                last_name: "Smith".to_owned(),
                affiliation: Some("Team Smith".to_owned()),
                gender: Gender::M,
                age: None,
                division: None,
            }
        );
        let part2 = Participant::from_ppl_record("0,Smith,John");
        assert!(part2.is_ok());
        assert_eq!(
            part2.unwrap(),
            Participant {
                chip_id: Vec::<String>::new(),
                bib: 0,
                first_name: "John".to_owned(),
                last_name: "Smith".to_owned(),
                affiliation: None,
                gender: Gender::X,
                age: None,
                division: None,
            }
        );
    }

    #[test]
    fn bad_bib() {
        let part = Participant::from_ppl_record("z,Smith,John,Team Smith,,M");
        assert!(part.is_err());
    }

    #[test]
    fn empty_record() {
        let part = Participant::from_ppl_record("");
        assert!(part.is_err());
    }
}
