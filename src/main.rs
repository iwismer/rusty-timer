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

use std::env;
use std::error::Error;
use std::path::Path;
mod chip_read;

fn read_file(path_str: String) -> Result<Vec<String>, String> {
    let path = Path::new(&path_str);

    let read_string = match std::fs::read_to_string(path) {
        Err(desc) => {
            return Err(format!(
                "couldn't read {}: {}",
                path.display(),
                desc.description()
            ))
        }
        Ok(s) => s,
    };
    Ok(read_string.split('\n').map(|s| s.to_string()).collect())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let path = &args[1];
    let reads = match read_file(path.to_string()) {
        Err(desc) => panic!("Error reading file {}", desc),
        Ok(reads) => reads,
    };
    // println!("{:?}", reads);
    for r in reads {
        if r != "" {
            let read = match chip_read::ChipRead::new(r.trim().to_string()) {
                Err(desc) => format!("Error reading chip {}", desc),
                Ok(read) => format!("{}", read),
            };
            println!("{}", read);
        }
    }
}
