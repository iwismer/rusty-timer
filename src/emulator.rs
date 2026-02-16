mod models;
mod util;
mod workers;
use models::{Message, ReadType};
use workers::{ClientConnector, ClientPool};

use crate::util::{is_delay, is_file, is_port, signal_handler};
use chrono::{Datelike, Timelike};
use clap::{Arg, Command};
use futures::{future::select_all, future::FutureExt, pin_mut};
use std::convert::TryFrom;
use std::fs::File;
use std::future::Future;
use std::io::{BufRead, BufReader, Lines};
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;
use tokio::sync::mpsc::{self, Sender};
use tokio::time::sleep;

fn generate_read(read_type: ReadType) -> String {
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
    match read_type {
        ReadType::RAW => format!("{}{:02x}", read, checksum),
        ReadType::FSLS => format!("{}{:02x}LS", read, checksum),
    }
}

async fn send_reads(
    delay: u64,
    mut file_reader: Option<Lines<BufReader<File>>>,
    bus_tx: Sender<Message>,
    read_type: ReadType,
) {
    loop {
        // Convert to string
        let mut chip_read: String = match file_reader.as_mut() {
            Some(lines) => match lines.next() {
                Some(line) => line.unwrap().trim().to_owned(),
                None => {
                    file_reader = None;
                    generate_read(read_type)
                }
            },
            None => generate_read(read_type),
        };
        chip_read.push_str("\r\n");
        // Send the read to the threads
        bus_tx
            .send(Message::CHIP_READ(chip_read))
            .await
            .unwrap_or_else(|_| {
                println!("\r\x1b[2KError sending read to thread. Maybe no readers are conected?");
            });
        sleep(Duration::from_millis(delay)).await;
    }
}

fn validate_port_value(value: &str) -> Result<u16, String> {
    is_port(value.to_owned())?;
    value
        .parse::<u16>()
        .map_err(|_| "Invalid port number".to_owned())
}

fn validate_delay_value(value: &str) -> Result<u64, String> {
    is_delay(value.to_owned())?;
    value
        .parse::<u64>()
        .map_err(|_| "Invalid delay value".to_owned())
}

fn validate_file_value(value: &str) -> Result<String, String> {
    is_file(value.to_owned())?;
    Ok(value.to_owned())
}

fn validate_read_type(value: &str) -> Result<ReadType, String> {
    ReadType::try_from(value).map_err(|_| "Invalid read type".to_owned())
}

#[tokio::main]
async fn main() {
    // Create the flags
    let matches = Command::new("Read Streamer")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Isaac Wismer")
        .about("A read streamer for timers")
        .arg(
            Arg::new("port")
                .help("The port of the local machine to listen for connections")
                .short('p')
                .long("port")
                .value_parser(validate_port_value)
                .default_value("10001"),
        )
        .arg(
            Arg::new("file")
                .help("The file to get the reads from")
                .short('f')
                .long("file")
                .value_parser(validate_file_value),
        )
        .arg(
            Arg::new("delay")
                .help("Delay between reads")
                .short('d')
                .long("delay")
                .value_parser(validate_delay_value)
                .default_value("1000"),
        )
        .arg(
            Arg::new("read_type")
                .help("The type of read the reader is sending")
                .short('t')
                .long("type")
                .value_parser(validate_read_type)
                .default_value("raw"),
        )
        .get_matches();

    // Check if the user has specified to save the reads to a file
    let mut file_reader: Option<Lines<BufReader<File>>> = None;
    if matches.contains_id("file") {
        // Create the file reader for saving reads
        let file_path = Path::new(matches.get_one::<String>("file").unwrap());
        file_reader = match File::open(file_path) {
            Ok(file) => Some(BufReader::new(file).lines()),
            Err(error) => {
                println!("Error opening file: {}", error);
                None
            }
        };
    }

    let delay = *matches
        .get_one::<u64>("delay")
        .expect("delay has a default value");
    let bind_port = *matches
        .get_one::<u16>("port")
        .expect("port has a default value");
    let read_type = *matches
        .get_one::<ReadType>("read_type")
        .expect("read_type has a default value");

    let (bus_tx, rx) = mpsc::channel::<Message>(1000);
    let client_pool = ClientPool::new(rx, None, None, false);
    let connector = ClientConnector::new(bind_port, bus_tx.clone()).await;

    let fut_clients = client_pool.begin().fuse();
    let fut_conn = connector.begin().fuse();
    let fut_sig = signal_handler().fuse();
    let fut_sender = send_reads(delay, file_reader, bus_tx.clone(), read_type).fuse();

    pin_mut!(fut_sender, fut_clients, fut_conn, fut_sig);
    let futures: Vec<Pin<&mut dyn Future<Output = ()>>> =
        vec![fut_sender, fut_clients, fut_conn, fut_sig];
    select_all(futures).await;
    // If any of them finish, end the program as something went wrong
    bus_tx.send(Message::SHUTDOWN).await.unwrap();
}
