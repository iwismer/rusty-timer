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

#[macro_use]
extern crate clap;
extern crate bus;
extern crate time;

use bus::Bus;
use clap::{App, Arg};
use std::fs::File;
use std::io::{BufRead, BufReader, Lines, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

type Port = u16;

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
        false => Err("File doesn't exists on file system! Use a different file".to_string()),
        true => Ok(()),
    }
}

fn is_delay(delay: String) -> Result<(), String> {
    match delay.parse::<u32>() {
        Ok(_) => Ok(()),
        Err(_) => Err("Invalid delay value".to_string()),
    }
}

fn generate_read() -> String {
    let now = time::now();
    format!(
        "aa00{}00{:>02}{:>02}{:>02}{:>02}{:>02}{:>02}{:>02}",
        "05800319aeeb0001",
        now.tm_year % 100,
        now.tm_mon,
        now.tm_mday,
        now.tm_hour,
        now.tm_min,
        now.tm_sec,
        now.tm_nsec / 1000000
    )
}

fn main() {
    // Create the flags
    let matches = App::new("Read Streamer")
        .version(crate_version!())
        .author("Isaac Wismer")
        .about("A read streamer for timers")
        .arg(
            Arg::with_name("port")
                .help("The port of the local machine to listen for connections")
                .short("p")
                .long("port")
                .takes_value(true)
                .validator(is_port)
                .default_value("10001"),
        )
        .arg(
            Arg::with_name("file")
                .help("The file to get the reads from")
                .short("f")
                .long("file")
                .takes_value(true)
                .validator(is_path),
        )
        .arg(
            Arg::with_name("delay")
                .help("Delay between reads")
                .short("d")
                .long("delay")
                .takes_value(true)
                .validator(is_delay),
        )
        .get_matches();

    // Check if the user has specified to save the reads to a file
    let mut file_reader: Option<Lines<BufReader<_>>> = None;
    if matches.is_present("file") {
        // Create the file reader for saving reads
        let file_path = Path::new(matches.value_of("file").unwrap());
        file_reader = match File::open(file_path) {
            Ok(file) => Some(BufReader::new(file).lines()),
            Err(error) => {
                println!("Error creating file: {}", error);
                process::exit(1);
            }
        };
    }

    let delay = matches
        .value_of("delay")
        .unwrap_or("1000")
        .parse::<u64>()
        .unwrap();

    // parse the port value
    // A port value of 0 let the OS assign a port
    let bind_port = matches.value_of("port").unwrap().parse::<Port>().unwrap();
    // Bind to the listening port to allow other computers to connect
    let listener = TcpListener::bind(("0.0.0.0", bind_port)).expect("Unable to bind to port");
    println!("Bound to port: {}", listener.local_addr().unwrap().port());
    // Create a bus to send the reads to the threads that control the connection
    // to each client computer
    let bus: Arc<Mutex<Bus<String>>> = Arc::new(Mutex::new(Bus::new(1000)));
    let bus_r = bus.clone();
    // Thread to connect to clients
    thread::spawn(move || {
        loop {
            // for stream in listener.incoming() {
            match listener.accept() {
                Ok((stream, addr)) => {
                    println!("Connected to client: {}", addr);
                    let mut rx = bus_r.lock().unwrap().add_rx();
                    thread::spawn(move || loop {
                        match stream
                            .try_clone()
                            .unwrap()
                            .write(rx.recv().unwrap().as_bytes())
                        {
                            Ok(_) => {}
                            Err(_) => {
                                println!("Error: Client {} disconnected unexpectedly", addr);
                                break;
                            }
                        };
                    });
                }
                Err(error) => {
                    println!("Failed to connect to client: {}", error);
                }
            }
            // }
        }
    });

    // Get reads
    loop {
        // Convert to string
        // println!("{:?}", now);
        let chip_read: String = match file_reader.as_mut() {
            Some(lines) => match lines.next() {
                Some(line) => line.unwrap(),
                None => generate_read(),
            },
            None => generate_read(),
        };
        // println!("{}", chip_read);
        // Lock the bus so I can send data along it
        let mut exclusive_bus = match bus.lock() {
            Ok(exclusive_bus) => exclusive_bus,
            Err(error) => {
                println!("Error communicating with thread: {}", error);
                continue;
            }
        };
        // Send the read to the threads
        match exclusive_bus.try_broadcast(chip_read.to_string()) {
            Ok(_) => (),
            Err(error) => println!(
                "Error sending read to thread. Maybe no readers are conected? {}",
                error
            ),
        }
        thread::sleep(Duration::from_millis(delay));
    }
}
