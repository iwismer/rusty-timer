#[macro_use]
extern crate clap;

use chrono::{Datelike, Timelike};
use clap::{App, Arg};
use futures::executor::block_on;
use std::fs::File;
use std::io::{BufRead, BufReader, Lines, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;
use tokio::sync::broadcast;

type Port = u16;
static CONNECTION_COUNT: AtomicUsize = AtomicUsize::new(0);

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
    let now = chrono::Local::now();
    let read = format!(
        "aa00{}{:>02}{:>02}{:>02}{:>02}{:>02}{:>02}{:>02}",
        "05800319aeeb0001",
        now.year() % 100,
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.nanosecond() / 10000000
    );
    let checksum = read[2..34].bytes().map(|b| b as u32).sum::<u32>() as u8;
    format!(
        "{}{:02x}",
        read,
        checksum
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
                .validator(is_delay)
                .default_value("1000"),
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

    let delay = matches.value_of("delay").unwrap().parse::<u64>().unwrap();

    let bind_port = matches.value_of("port").unwrap().parse::<Port>().unwrap();
    let listener = TcpListener::bind(("0.0.0.0", bind_port)).expect("Unable to bind to port");
    println!("Bound to port: {}", listener.local_addr().unwrap().port());
    // Create a bus to send the reads to the threads that control the connection
    // to each client computer
    let (sender, _) = broadcast::channel::<String>(1000);
    let sender_clone = sender.clone();
    // Thread to connect to clients
    thread::spawn(move || {
        loop {
            match listener.accept() {
                Ok((stream, addr)) => {
                    CONNECTION_COUNT.fetch_add(1, Ordering::SeqCst);
                    println!("Connected to client: {}", addr);
                    let mut rx = sender_clone.subscribe();
                    thread::spawn(move || {
                        loop {
                            match stream
                                .try_clone()
                                .unwrap()
                                .write(block_on(rx.recv()).unwrap().as_bytes())
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
        }
    });

    // Get reads
    loop {
        if CONNECTION_COUNT.load(Ordering::SeqCst) > 0 {
            // Convert to string
            let mut chip_read: String = match file_reader.as_mut() {
                Some(lines) => match lines.next() {
                    Some(line) => line.unwrap().trim().to_string(),
                    None => generate_read(),
                },
                None => generate_read(),
            };
            chip_read.push_str("\r\n");
            // Send the read to the threads
            match sender.clone().send(chip_read.to_string()) {
                Ok(_) => (),
                Err(_) => println!("Error sending read to thread. Maybe no readers are connected?"),
            }
            // println!("{} {:?} {:?}", chip_read.len(), chip_read, chip_read.as_bytes());
        }
        thread::sleep(Duration::from_millis(delay));
    }
}
