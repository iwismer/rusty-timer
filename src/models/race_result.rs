use super::{ChipRead, Participant};
use std::fmt;

#[derive(Debug)]
pub struct RaceResult {
    pub participant: Participant,
    pub start_reads: Vec<ChipRead>,
    // TODO: Make a Vec of Vec to allow for multiple timing points
    pub finish_reads: Vec<ChipRead>,
}

#[allow(dead_code)]
impl RaceResult {
    // Create a new race result with no reads
    pub fn new(participant: Participant) -> RaceResult {
        RaceResult {
            participant,
            start_reads: Vec::new(),
            finish_reads: Vec::new(),
        }
    }

    // Create a new race result with chip reads
    pub fn new_with_reads(
        participant: Participant,
        start_reads: Vec<ChipRead>,
        finish_reads: Vec<ChipRead>,
    ) -> RaceResult {
        RaceResult {
            participant,
            start_reads,
            finish_reads,
        }
    }

    // Add a new chip start read
    pub fn add_start_read(&mut self, read: ChipRead) {
        self.start_reads.push(read);
    }

    // Add a new chip finish read
    pub fn add_finish_read(&mut self, read: ChipRead) {
        self.finish_reads.push(read);
    }

    // Sort the reads ib the result
    pub fn sort_reads(&mut self) {
        self.start_reads.sort_unstable_by_key(|r| r.timestamp);
        self.finish_reads.sort_unstable_by_key(|r| r.timestamp);
    }
}

impl fmt::Display for RaceResult {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Result (Participant: {} Start Time: {} Finish Time: {})",
            self.participant,
            match self.start_reads.len() {
                len if len > 0 => self.start_reads[len - 1].timestamp.time_string(),
                _ => "No start time".to_owned(),
            },
            match self.finish_reads.len() {
                len if len > 0 => self.finish_reads[0].timestamp.time_string(),
                _ => "No finish time".to_owned(),
            },
        )
    }
}
