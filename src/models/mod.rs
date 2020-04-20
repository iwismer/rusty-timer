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

pub mod chip;
pub mod participant;
pub mod race_result;
pub mod timestamp;

pub type ChipBib = chip::ChipBib;
pub type ChipRead = chip::ChipRead;
pub type Participant = participant::Participant;
pub type Gender = participant::Gender;
pub type Timestamp = timestamp::Timestamp;
pub type RaceResult = race_result::RaceResult;
