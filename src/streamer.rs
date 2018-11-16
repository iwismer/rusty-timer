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

use bus::Bus;
use clap::{App, Arg};
use std::fs::File;
// use std::io;
use std::io::{Read, Write};
use std::net::Ipv4Addr;
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
mod chip_read;

type Port = u16;

static CONNECTION_COUNT: AtomicUsize = AtomicUsize::new(0);

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

fn main() {
    // Create the flags
    let matches = App::new("Read Streamer")
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
                .help("The port of the local machine to listen for connections")
                .short("p")
                .long("port")
                .takes_value(true)
                .validator(is_port)
                .default_value("10001"),
        )
        .arg(
            Arg::with_name("file")
                .help("The file to output the reads to")
                .short("f")
                .long("file")
                .takes_value(true)
                .validator(is_path),
        )
        .get_matches();

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
    // Bind to the listening port to allow other computers to connect
    let listener = TcpListener::bind(("0.0.0.0", bind_port)).expect("Unable to bind to port");
    println!("Bound to port: {}", listener.local_addr().unwrap().port());
    println!("Waiting for reader: {}:{}", reader, 10000);
    // Connect to the reader
    let mut stream = match TcpStream::connect((reader, 10000)) {
        Ok(stream) => {
            println!("Connected to reader: {}:{}", reader, 10000);
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
            // for stream in listener.incoming() {
            match listener.accept() {
                Ok((stream, addr)) => {
                    CONNECTION_COUNT.fetch_add(1, Ordering::SeqCst);
                    println!("Connected to client: {}", addr);
                    let mut rx = bus_r.lock().unwrap().add_rx();
                    thread::spawn(move || {
                        loop {
                            match stream
                                .try_clone()
                                .unwrap()
                                .write(rx.recv().unwrap().as_bytes())
                            {
                                Ok(_) => {}
                                Err(_) => {
                                    println!("Warning: Client {} disconnected unexpectedly", addr);
                                    break;
                                }
                            };
                        }
                        CONNECTION_COUNT.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(error) => {
                    println!("Failed to connect to client: {}", error);
                }
            }
            // }
        }
    });

    // Get 38 bytes from the stream, which is exactly 1 read
    let mut input_buffer = [0u8; 38];
    let mut read_count: u32 = 0;
    loop {
        match stream.read_exact(&mut input_buffer) {
            Ok(_) => {
                read_count += 1;
                // io::stdout().write(&input_buffer).unwrap();
                // io::stdout().flush().unwrap();
                // Convert to string
                let read = match std::str::from_utf8(&input_buffer) {
                    Ok(read) => read,
                    Err(error) => {
                        println!("Error parsing chip read: {}", error);
                        continue;
                    }
                };
                // print!("{}", read);
                // Only write to file if a file was supplied
                if file_writer.is_some() {
                    match write!(
                        // This unwrap is safe as file_writer has been
                        // proven to be Some(T)
                        file_writer.as_mut().unwrap(),
                        "{}{}",
                        read.replace(|c: char| !c.is_alphanumeric(), ""),
                        // Use \r\n on a windows machine
                        line_ending
                    ) {
                        Ok(_) => (),
                        Err(error) => println!("Error writing read to file: {}", error),
                    };
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
                    match exclusive_bus.try_broadcast(read.to_string()) {
                        Ok(_) => (),
                        Err(error) => println!(
                            "Error sending read to thread. Maybe no readers are conected? {}",
                            error
                        ),
                    }
                }
                match chip_read::ChipRead::new(read.to_string()) {
                    Err(desc) => println!("Error reading chip {}", desc),
                    Ok(read) => {
                        print!(
                            "Total Reads: {} Last Read: {} {}\r",
                            read_count, read.tag_id, read.timestamp
                        );
                    }
                };
            }
            Err(error) => {
                println!("Error reading from reader: {}", error);
            }
        }
    }
}
