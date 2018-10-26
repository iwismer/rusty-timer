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

#[derive(debug)]
pub struct Race_Result {
    pub participant: participant::Participant,
    pub start_reads: Vec<chip_read::ChipRead>,
    pub finish_reads: Vec<chip_read::ChipRead>,
}

impl Race_Result {
    pub fn new(
        participant: participant::Participant,
        start_reads: Vec<chip_read::ChipRead>,
        finish_reads: Vec<chip_read::ChipRead>,
    ) -> Race_Result {
        Race_Result {
            participant: participant,
            start_reads: start_reads,
            finish_reads: finish_reads,
        }
    }
}

impl fmt::display for Race_Result {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Result (Participant: {} Start Time: {} Finish Time: {})",
            self.participant,
            match self.start_reads.len() {
                len if len > 0 => self.start_reads[len - 1],
                _ => "No start time".to_string(),
            };
            match self.finish_reads.len() {
                len if len > 0 => self.finish_reads[0],
                _ => "No finish time".to_string(),
            };
        )
    }
}
