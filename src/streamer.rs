/*
Copyright © 2019  Isaac Wismer

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

#[macro_use]
extern crate clap;
extern crate bus;
extern crate encoding;
extern crate rusqlite;

use bus::Bus;
use clap::{App, Arg};
use encoding::all::WINDOWS_1252;
use encoding::{DecoderTrap, Encoding};
use rusqlite::types::ToSql;
use rusqlite::{Connection, NO_PARAMS};
use std::error::Error;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::Ipv4Addr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

mod chip_read;
mod participant;
use chip_read::ChipRead;
use participant::{Gender, Participant};

type Port = u16;

static CONNECTION_COUNT: AtomicUsize = AtomicUsize::new(0);

pub struct Chip {
    pub id: String,
    pub bib: i32,
}

fn read_file(path_str: &String) -> Result<Vec<String>, String> {
    let path = Path::new(path_str);
    match std::fs::read_to_string(path) {
        Err(_desc) => match std::fs::read(path) {
            Err(desc) => Err(format!(
                "couldn't read {}: {}",
                path.display(),
                desc.description()
            )),
            Ok(mut buffer) => Ok(WINDOWS_1252
                .decode(buffer.as_mut_slice(), DecoderTrap::Replace)
                .unwrap()),
        },
        Ok(s) => Ok(s),
    }
    .map(|s| s.split('\n').map(|s| s.to_string()).collect())
}

fn get_bibchip(file_path: String) -> Vec<Chip> {
    // Attempt to read the bibs, assuming UTF-8 file encoding
    let bibs = match read_file(&file_path) {
        // If there was an error, attempt again with Windows 1252 encoding
        Err(desc) => {
            println!("Error reading bibchip file {}", desc);
            Vec::new()
        }
        Ok(bibs) => bibs,
    };
    // parse the file and import bib chips into hashmap
    let mut bib_chip = Vec::new();
    for b in bibs {
        if b != "" && b.chars().next().unwrap().is_digit(10) {
            let parts = b.trim().split(",").collect::<Vec<&str>>();
            bib_chip.push(Chip {
                id: parts[1].to_string(),
                bib: parts[0].parse::<i32>().unwrap_or_else(|_| {
                    println!("Error reading bib file. Invalid bib: {}", parts[0]);
                    0
                }),
            });
        }
    }
    bib_chip
}

fn get_participants(ppl_path: String) -> Vec<Participant> {
    // Attempt to read the participants, assuming UTF-8 file encoding
    let ppl = match read_file(&ppl_path) {
        // If there was an error, attempt again with Windows 1252 encoding
        Err(desc) => {
            println!("Error reading participant file {}", desc);
            Vec::new()
        },
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

// Check if the string is a valid IPv4 address
fn is_ip_addr(ip: String) -> Result<(), String> {
    match ip.parse::<Ipv4Addr>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid IP Address".to_string()),
    }
}

// Check if the string is a valid port
fn is_port(port: String) -> Result<(), String> {
    match port.parse::<Port>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid port number".to_string()),
    }
}

// Check that the path does not already point to a file
fn is_path(path_str: String) -> Result<(), String> {
    let path = Path::new(&path_str);
    match path.exists() {
        true => Err("File exists on file system! Use a different file".to_string()),
        false => Ok(()),
    }
}

// Check that the path does not already point to a file
fn is_file(file_str: String) -> Result<(), String> {
    let path = Path::new(&file_str);
    match path.exists() {
        true => Ok(()),
        false => Err("File doesn't exists on file system! Use a different file".to_string()),
    }
}

fn main() {
    // Create the flags
    let matches = App::new("Rusty Timer: Read Streamer")
        .version(crate_version!())
        .author("Isaac Wismer")
        .about("A read streamer for timers")
        .arg(
            Arg::with_name("reader")
                .help("The IP address of the reader to connect to")
                .index(1)
                .takes_value(true)
                .value_name("reader_ip")
                .validator(is_ip_addr)
                .required(true),
        )
        .arg(
            Arg::with_name("port")
                .help("The port of the local machine to bind to")
                .short("p")
                .long("port")
                .takes_value(true)
                .validator(is_port)
                .default_value("10001"),
        )
        .arg(
            Arg::with_name("reader-port")
                .help("The port of the reader to connect to")
                .short("r")
                .long("reader-port")
                .takes_value(true)
                .validator(is_port)
                .default_value("10000"),
        )
        .arg(
            Arg::with_name("file")
                .help("The file to output the reads to")
                .short("f")
                .long("file")
                .takes_value(true)
                .validator(is_path),
        )
        .arg(
            Arg::with_name("bibchip")
                .help("The bib-chip file")
                .short("b")
                .long("bibchip")
                .takes_value(true)
                .validator(is_file),
        )
        .arg(
            Arg::with_name("participants")
                .help("The .ppl participant file")
                .short("P")
                .long("ppl")
                .takes_value(true)
                .validator(is_file)
                .requires("bibchip"),
        )
        .arg(
            Arg::with_name("is_buffered")
                .help("Buffer the output. Use if high CPU use in encountered")
                .short("B")
                .long("buffer")
                .takes_value(false),
        )
        .get_matches();

    let conn = Connection::open_in_memory().unwrap();
    conn.execute(
        "CREATE TABLE participant (
                  bib           INTEGER PRIMARY KEY,
                  first_name    TEXT NOT NULL,
                  last_name     TEXT NOT NULL,
                  gender        CHECK( gender IN ('M','F','X') ) NOT NULL DEFAULT 'X',
                  affiliation   TEXT,
                  division      INTEGER
                  )",
        NO_PARAMS,
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE chip (
                  id     TEXT PRIMARY KEY,
                  bib    INTEGER NOT NULL
                  )",
        NO_PARAMS,
    )
    .unwrap();

    // Check if there was a bibchip argument
    if matches.is_present("bibchip") {
        // Unwrap is sage as bibchip argument is present
        let path = matches.value_of("bibchip").unwrap();
        let bib_chips = get_bibchip(path.to_string());
        // println!("{:?}", bib_chip_map);
        for c in &bib_chips {
            conn.execute(
                "INSERT INTO chip (id, bib)
                        VALUES (?1, ?2)",
                &[&c.id as &dyn ToSql, &c.bib],
            )
            .unwrap();
        }
    }
    if matches.is_present("participants") {
        // Unwrap is sage as participants argument is present
        let path = matches.value_of("participants").unwrap();
        let participants = get_participants(path.to_string());
        for p in &participants {
            conn.execute(
                "INSERT INTO participant (bib, first_name, last_name, gender, affiliation, division)
                        VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                &[
                    &p.bib as &dyn ToSql,
                    &p.first_name as &dyn ToSql,
                    &p.last_name as &dyn ToSql,
                    &format!("{}", p.gender),
                    &p.affiliation as &dyn ToSql,
                    &p.division as &dyn ToSql,
                ],
            )
            .unwrap();
        }
    }

    let is_buffered = matches.is_present("is_buffered");

    // Check if the user has specified to save the reads to a file
    let mut file_writer: Option<File> = None;
    if matches.is_present("file") {
        // Create the file writer for saving reads
        let file_path = Path::new(matches.value_of("file").unwrap());
        file_writer = match File::create(file_path) {
            Ok(file) => Some(file),
            Err(error) => {
                println!("Error creating file: {}", error);
                process::exit(1);
            }
        };
    }

    // Check if running on windows
    let line_ending = match cfg!(windows) {
        true => "\r\n",
        false => "\n",
    };
    // Get the address of the reader and parse to IP
    let reader = matches
        .value_of("reader")
        .unwrap()
        .parse::<Ipv4Addr>()
        .unwrap();
    // parse the port value
    // A port value of 0 let the OS assign a port
    let bind_port = matches.value_of("port").unwrap().parse::<Port>().unwrap();
    let reader_port = matches
        .value_of("reader-port")
        .unwrap()
        .parse::<Port>()
        .unwrap();
    // Bind to the listening port to allow other computers to connect
    let listener = TcpListener::bind(("0.0.0.0", bind_port)).expect("Unable to bind to port");
    println!("Bound to port: {}", listener.local_addr().unwrap().port());
    println!("Waiting for reader: {}:{}", reader, reader_port);
    // Connect to the reader
    let mut stream = match TcpStream::connect((reader, reader_port)) {
        Ok(stream) => {
            println!("Connected to reader: {}:{}", reader, reader_port);
            stream
        }
        Err(error) => {
            println!("Failed to connect to reader: {}", error);
            process::exit(1);
        }
    };
    // Create a bus to send the reads to the threads that control the connection
    // to each client computer
    let bus: Arc<Mutex<Bus<String>>> = Arc::new(Mutex::new(Bus::new(1000)));
    let bus_r = bus.clone();
    // Thread to connect to clients
    thread::spawn(move || {
        loop {
            // wait for a connection, then connect when it comes
            match listener.accept() {
                Ok((stream, addr)) => {
                    // Increment the number of connections
                    CONNECTION_COUNT.fetch_add(1, Ordering::SeqCst);
                    println!("Connected to client: {}", addr);
                    // Add a receiver for the connection
                    let mut rx = bus_r.lock().unwrap().add_rx();
                    // Make a thread for that connection
                    thread::spawn(move || {
                        loop {
                            // Receive messages and pass to client
                            match stream
                                .try_clone()
                                .unwrap()
                                .write(rx.recv().unwrap().as_bytes())
                            {
                                Ok(_) => {}
                                Err(_) => {
                                    println!("Warning: Client {} disconnected", addr);
                                    // Decrement the number of clients on disconnect
                                    CONNECTION_COUNT.fetch_sub(1, Ordering::SeqCst);
                                    // end the loop, destroying the thread
                                    break;
                                }
                            };
                        }
                    });
                }
                Err(error) => {
                    println!("Failed to connect to client: {}", error);
                }
            }
        }
    });

    // Get 38 bytes from the stream, which is exactly 1 read
    let mut input_buffer = [0u8; 38];
    let mut read_count: u32 = 0;
    loop {
        match stream.read_exact(&mut input_buffer) {
            Ok(_) => (),
            Err(e) => {
                println!("Error reading from reader: {}", e);
                continue;
            }
        }
        read_count += 1;
        // Convert to string
        let read = match std::str::from_utf8(&input_buffer) {
            Ok(read) => read,
            Err(error) => {
                println!("Error parsing chip read: {}", error);
                continue;
            }
        };
        // Only write to file if a file was supplied
        if file_writer.is_some() {
            write!(
                // This unwrap is safe as file_writer has been
                // proven to be Some(T)
                file_writer.as_mut().unwrap(),
                "{}{}",
                read.replace(|c: char| !c.is_alphanumeric(), ""),
                // Use \r\n on a windows machine
                line_ending
            )
            .unwrap_or_else(|e| {
                println!("Error writing read to file: {}", e);
            });
        }
        // Check that there is a connection
        if CONNECTION_COUNT.load(Ordering::SeqCst) > 0 {
            // Lock the bus so I can send data along it
            let mut exclusive_bus = match bus.lock() {
                Ok(exclusive_bus) => exclusive_bus,
                Err(error) => {
                    println!("Error communicating with thread: {}", error);
                    continue;
                }
            };
            // Send the read to the threads
            exclusive_bus
                .try_broadcast(read.to_string())
                .unwrap_or_else(|e| {
                    println!(
                        "Error sending read to thread. Maybe no readers are conected? {}",
                        e
                    )
                });
        }
        match ChipRead::new(read.to_string()) {
            Err(desc) => println!("Error reading chip {}", desc),
            Ok(read) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT
                                c.id,
                                c.bib,
                                p.first_name,
                                p.last_name
                                FROM chip c
                                LEFT JOIN participant p
                                ON c.bib = p.bib
                                WHERE c.id = ?",
                    )
                    .unwrap();
                // Make the query and map to a participant
                let row = stmt.query_row(&[read.tag_id.as_str()], |row| {
                    Ok(Participant {
                        // If there is a missing field, then map it to unknown
                        chip_id: vec![row.get(0).unwrap_or("None".to_string())],
                        bib: row.get(1).unwrap_or(0),
                        first_name: row.get(2).unwrap_or("Unknown".to_string()),
                        last_name: row.get(3).unwrap_or("Participant".to_string()),
                        gender: Gender::X,
                        age: None,
                        affiliation: None,
                        division: None,
                    })
                });

                match row {
                    // Bandit chip
                    Err(_) => {
                        print!(
                            "Total Reads: {} Last Read: Unknown Chip {} {}\r",
                            read_count,
                            read.tag_id,
                            read.time_string()
                        );
                    }
                    // Good chip, either good or unknown participant
                    Ok(participant) => {
                        // println!("{:?}", participant);
                        print!(
                            "Total Reads: {} Last Read: {} {} {} {}\r",
                            read_count,
                            participant.bib,
                            participant.first_name,
                            participant.last_name,
                            read.time_string()
                        );
                    }
                }
                // only flush if the output is unbuffered
                // This can cause high CPU use on some systems
                if !is_buffered {
                    io::stdout().flush().unwrap_or(());
                }
            }
        };
    }
}
