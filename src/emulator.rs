#[macro_use]
extern crate clap;

mod models;
mod util;
mod workers;
use models::Message;
use workers::{ClientConnector, ClientPool};

use crate::util::{is_delay, is_path, is_port, signal_handler};
use chrono::{Datelike, Timelike};
use clap::{App, Arg};
use futures::{future::select_all, future::Future, future::FutureExt, pin_mut};
use std::fs::File;
use std::io::{BufRead, BufReader, Lines};
use std::path::Path;
use std::pin::Pin;
use std::process;
use std::time::Duration;
use tokio::sync::mpsc::{self, Sender};
use tokio::time::delay_for;

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
    format!("{}{:02x}", read, checksum)
}

async fn send_reads(
    delay: u64,
    mut file_reader: Option<Lines<BufReader<File>>>,
    mut bus_tx: Sender<Message>,
) {
    loop {
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
        bus_tx
            .send(Message::CHIP_READ(chip_read.to_string()))
            .await
            .unwrap_or_else(|_| {
                println!("\r\x1b[2KError sending read to thread. Maybe no readers are conected?");
            });
        // println!("{} {:?} {:?}", chip_read.len(), chip_read, chip_read.as_bytes());
        delay_for(Duration::from_millis(delay)).await;
    }
}

#[tokio::main]
async fn main() {
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
    let mut file_reader: Option<Lines<BufReader<File>>> = None;
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
    let bind_port = matches.value_of("port").unwrap().parse::<u16>().unwrap();

    let (bus_tx, rx) = mpsc::channel::<Message>(1000);
    let client_pool = ClientPool::new(rx, None, None, false);
    let connector = ClientConnector::new(bind_port, bus_tx.clone()).await;

    let fut_clients = client_pool.begin().fuse();
    let fut_conn = connector.begin().fuse();
    let fut_sig = signal_handler().fuse();
    let fut_sender = send_reads(delay, file_reader, bus_tx.clone()).fuse();

    pin_mut!(fut_sender, fut_clients, fut_conn, fut_sig);
    let futures: Vec<Pin<&mut dyn Future<Output = ()>>> =
        vec![fut_sender, fut_clients, fut_conn, fut_sig];
    select_all(futures).await;
    // If any of them finish, end the program as something went wrong
    bus_tx.clone().send(Message::SHUTDOWN).await.unwrap();
}
