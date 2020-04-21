pub mod chip;
pub mod message;
pub mod participant;
pub mod race_result;
pub mod timestamp;

#[allow(dead_code)]
pub type ChipBib = chip::ChipBib;
pub type ChipRead = chip::ChipRead;
pub type Participant = participant::Participant;
pub type Gender = participant::Gender;
pub type Timestamp = timestamp::Timestamp;
#[allow(dead_code)]
pub type RaceResult = race_result::RaceResult;
pub type Message = message::Message;
