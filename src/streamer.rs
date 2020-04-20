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

#[macro_use]
extern crate clap;
extern crate bus;
extern crate encoding;
extern crate rusqlite;

use futures::executor::block_on;
use bus::Bus;
use clap::{App, Arg};
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
use signal_hook::{iterator::Signals, SIGINT};
use std::net::Shutdown;

mod util;
mod client;
mod models;
use client::Client;
use models::{Gender, Participant, ChipRead, ChipBib};
use models::chip::read_bibchip_file;
use models::participant::read_participant_file;

type Port = u16;

static CONNECTION_COUNT: AtomicUsize = AtomicUsize::new(0);


/// Check if the string is a valid IPv4 address
fn is_ip_addr(ip: String) -> Result<(), String> {
    match ip.parse::<Ipv4Addr>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid IP Address".to_string()),
    }
}

/// Check if the string is a valid port
fn is_port(port: String) -> Result<(), String> {
    match port.parse::<Port>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid port number".to_string()),
    }
}

/// Check that the path does not already point to a file
fn is_path(path_str: String) -> Result<(), String> {
    let path = Path::new(&path_str);
    match path.exists() {
        true => Err("File exists on file system! Use a different file".to_string()),
        false => Ok(()),
    }
}

/// Check that the path does not already point to a file
fn is_file(file_str: String) -> Result<(), String> {
    let path = Path::new(&file_str);
    match path.exists() {
        true => Ok(()),
        false => Err("File doesn't exists on file system! Use a different file".to_string()),
    }
}

fn read_to_string(read: &str, conn: &rusqlite::Connection, read_count: &u32) -> String {
    match ChipRead::new(read.to_string()) {
        Err(desc) => format!("Error reading chip {}", desc),
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
                Err(_) => format!(
                    "Total Reads: {} Last Read: Unknown Chip {} {}",
                    read_count,
                    read.tag_id,
                    read.time_string()
                ),
                // Good chip, either good or unknown participant
                Ok(participant) => {
                    // println!("{:?}", participant);
                    format!(
                        "Total Reads: {} Last Read: {} {} {} {}",
                        read_count,
                        participant.bib,
                        participant.first_name,
                        participant.last_name,
                        read.time_string()
                    )
                }
            }
        }
    }
}

async fn main_async() {
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
        let bib_chips = read_bibchip_file(path.to_string());
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
        // Unwrap is safe as participants argument is present
        let path = matches.value_of("participants").unwrap();
        let participants = read_participant_file(path.to_string());
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

    let handler_bus: Arc<Mutex<Bus<bool>>> = Arc::new(Mutex::new(Bus::new(1000)));
    let handler_bus_r = handler_bus.clone();
    let stream_handler = stream.try_clone().unwrap();
    thread::spawn(move || {
        let signals = Signals::new(&[SIGINT]).unwrap();
        for sig in signals.forever() {
            println!("\r\x1b[2KReceived signal {:?}", sig);
            handler_bus.lock().unwrap().try_broadcast(true).unwrap();
            stream_handler.shutdown(Shutdown::Both).unwrap();
        }
    });
    // let mut clients = Vec::new();
    // Thread to connect to clients
    thread::spawn(move || {
        loop {
            // wait for a connection, then connect when it comes
            match listener.accept() {
                Ok((stream, addr)) => {
                    // Increment the number of connections
                    CONNECTION_COUNT.fetch_add(1, Ordering::SeqCst);
                    // Add a receiver for the connection
                    let rx = bus_r.lock().unwrap().add_rx();
                    let handler_rx = handler_bus_r.lock().unwrap().add_rx();
                    match Client::new(stream, addr, rx, handler_rx, || {
                        CONNECTION_COUNT.fetch_sub(1, Ordering::SeqCst);
                    }) {
                        Err(_) => eprintln!("\r\x1b[2KError connecting to client"),
                        Ok(client) => {
                            thread::spawn(|| {
                                let c = client.begin();
                                block_on(c);
                            });
                            // clients.push(client);
                            println!("\r\x1b[2KConnected to client: {}", addr)
                        }
                    };
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
                println!("\r\x1b[2KError reading from reader: {}", e);
                if e.kind() == io::ErrorKind::UnexpectedEof {
                    println!("Reader disconnected!");
                    process::exit(1);
                }
                continue;
            }
        }
        read_count += 1;
        // Convert to string
        let read = match std::str::from_utf8(&input_buffer) {
            Ok(read) => read,
            Err(error) => {
                println!("\r\x1b[2KError parsing chip read: {}", error);
                continue;
            }
        };
        // println!("'{}'", read);
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
                println!("\r\x1b[2KError writing read to file: {}", e);
            });
        }
        // Check that there is a connection
        if CONNECTION_COUNT.load(Ordering::SeqCst) > 0 {
            // Lock the bus so I can send data along it
            let mut exclusive_bus = match bus.lock() {
                Ok(exclusive_bus) => exclusive_bus,
                Err(error) => {
                    println!("\r\x1b[2KError communicating with thread: {}", error);
                    continue;
                }
            };
            // Send the read to the threads
            exclusive_bus
                .try_broadcast(read.to_string())
                .unwrap_or_else(|e| {
                    println!(
                        "\r\x1b[2KError sending read to thread. Maybe no readers are conected? {}",
                        e
                    )
                });
        }
        let to_print = read_to_string(&read, &conn, &read_count);
        print!("\r\x1b[2K{}", to_print);
        // only flush if the output is unbuffered
        // This can cause high CPU use on some systems
        if !is_buffered {
            io::stdout().flush().unwrap_or(());
        }
    }
}

fn main() {
    block_on(main_async());
}
