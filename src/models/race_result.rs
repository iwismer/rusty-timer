/*
Copyright © 2020  Isaac Wismer

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
use super::{Participant, ChipRead};

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
            participant: participant,
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
            participant: participant,
            start_reads: start_reads,
            finish_reads: finish_reads,
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
