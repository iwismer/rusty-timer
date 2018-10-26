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

#![allow(unused_variables)]
#![allow(unused_imports)]

use std::io::{self, Read, Write};
use std::net::TcpListener;
use std::net::TcpStream;
use std::thread;

use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::path::Path;
use std::process;
mod chip_read;
mod participant;

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
    // Get args
    let args: Vec<String> = env::args().collect();
    // check number of args
    if args.len() != 4 {
        println!("Incorrect number of arguments");
        process::exit(1);
    }
    // path to the chip reads file
    let path = &args[1];
    // Read into list of lines
    let reads = match read_file(path.to_string()) {
        Err(desc) => panic!("Error reading file {}", desc),
        Ok(reads) => reads,
    };
    // import reads
    let mut chip_reads = Vec::new();
    for r in reads {
        if r != "" {
            match chip_read::ChipRead::new(r.trim().to_string()) {
                Err(desc) => println!("Error reading chip {}", desc),
                Ok(read) => chip_reads.push(read),
            };
        }
    }
    // path of bib chip file
    let path = &args[3];
    let bibs = match read_file(path.to_string()) {
        Err(desc) => panic!("Error reading file {}", desc),
        Ok(bibs) => bibs,
    };
    // Import bib chips into hashmap
    let mut bib_chip = HashMap::new();
    for b in bibs {
        if b != "" && b.chars().next().unwrap().is_digit(10) {
            let parts = b.trim().split(",").collect::<Vec<&str>>();
            bib_chip.insert(parts[0].parse::<i32>().unwrap(), parts[1].to_string());
        }
    }
    // path to List of people
    let path = &args[2];
    let ppl = match read_file(path.to_string()) {
        Err(desc) => panic!("Error reading file {}", desc),
        Ok(ppl) => ppl,
    };
    // Read into list of participants and add the chip
    let mut participants = Vec::new();
    for p in ppl {
        if p != "" && !p.starts_with(";") {
            match participant::Participant::from_ppl_record(p.trim().to_string()) {
                Err(desc) => println!("Error reading person {}", desc),
                Ok(mut person) => {
                    // println!("{}", person);
                    match bib_chip.get(&person.bib) {
                        Some(id) => person.chip_id.push(id.to_string()),
                        None => (),
                    }
                    participants.push(person);
                }
            };
        }
    }
    println!("{:?}", participants[0].chip_id);
    println!("{}", chip_reads[0].tag_id);
    for read in chip_reads {
        // println!("{} {:?}", read.tag_id, participants.iter().find(|p| p.chip_id.contains(&read.tag_id)));
        if participants[0].chip_id.contains(&read.tag_id) {
            println!("Found!");
        }
        println!(
            "{} {}",
            read,
            match participants
                .iter()
                .find(|p| p.chip_id.contains(&read.tag_id))
            {
                Some(participant) => format!("{}", participant),
                None => "No Participant Found".to_string(),
            }
        );
    }
    // let mut input_stream = TcpStream::connect(("192.168.0.154", 10000)).unwrap();
    // println!("Connected!");
    // // let handler = thread::spawn(move || {
    //     let mut client_buffer = [0u8; 1024];

    //     loop {
    //         match input_stream.read(&mut client_buffer) {
    //             Ok(n) => {
    //                 if n == 0 {
    //                     process::exit(1);
    //                 } else {
    //                     // io::stdout().write(&client_buffer).unwrap();
    //                     // io::stdout().flush().unwrap();
    //                     let chip_read = std::str::from_utf8(&client_buffer);
    //                     match chip_read {
    //                         Err(err) => println!("{}", err),
    //                         Ok(string) => {
    //                             match chip_read::ChipRead::new(string.trim().to_string()) {
    //                                 Err(desc) => println!("Error reading chip {}", desc),
    //                                 Ok(read) => {
    //                                     // if participants[0].chip_id.contains(&read.tag_id) {
    //                                     //     println!("Found!");
    //                                     // }
    //                                     println!(
    //                                         "{} {}",
    //                                         read,
    //                                         match participants
    //                                             .iter()
    //                                             .find(|p| p.chip_id.contains(&read.tag_id))
    //                                         {
    //                                             Some(participant) => format!("{}", participant),
    //                                             None => "No Participant Found".to_string(),
    //                                         }
    //                                     );
    //                                 },
    //                             };
    //                         },
    //                     }
    //                 }
    //             }
    //             Err(error) => println!("{}", error.to_string()),
    //         }
    //     }
    // });

    // let output_stream = &mut stream;
    // let mut user_buffer = String::new();

    // loop {
    //     io::stdin().read_line(&mut user_buffer).unwrap();

    //     output_stream.write(user_buffer.as_bytes()).unwrap();
    //     output_stream.flush().unwrap();
    // }
}
