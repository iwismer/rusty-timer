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

use clap::{App, Arg};
use futures::{future::FutureExt, join};
use rusqlite::types::ToSql;
use rusqlite::{Connection, NO_PARAMS};
use signal_hook::{iterator::Signals, SIGINT};
use std::net::Ipv4Addr;
use std::net::Shutdown;
use std::path::Path;
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::thread;
use tokio::sync::broadcast;
use tokio::sync::mpsc;

mod models;
mod util;
mod workers;
use models::Message;
use models::chip::read_bibchip_file;
use models::participant::read_participant_file;
use workers::{ClientConnector, ReadBroadcaster, ClientPool};

type Port = u16;

pub static CONNECTION_COUNT: AtomicUsize = AtomicUsize::new(0);

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

struct Args {
    bib_chip_file_path: Option<String>,
    participants_file_path: Option<String>,
    reader_ip: Ipv4Addr,
    reader_port: Port,
    bind_port: Port,
    out_file: Option<String>,
    buffered_output: bool,
}

#[tokio::main]
async fn main() {
    let args = get_args();

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
    if args.bib_chip_file_path.is_some() {
        // Unwrap is sage as bibchip argument is present
        let bib_chips = read_bibchip_file(args.bib_chip_file_path.unwrap().to_string());
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
    if args.participants_file_path.is_some() {
        // Unwrap is safe as participants argument is present
        let participants = read_participant_file(args.participants_file_path.unwrap().to_string());
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

    // Create a bus to send the reads to the threads that control the connection
    // to each client computer
    let (chip_read_tx, rx) = mpsc::channel::<Message>(1000);
    // let (signal_tx, _) = broadcast::channel::<bool>(10);

    let connector = ClientConnector::new(args.bind_port, chip_read_tx.clone()).await;
    let receiver = ReadBroadcaster::new(
        args.reader_ip,
        args.reader_port,
        chip_read_tx.clone(),
        conn,
        args.out_file,
        args.buffered_output,
    )
    .await;
    let client_pool = ClientPool::new(rx);

    // let stream_handler = receiver.get_stream_clone();
    // tokio::spawn(async {
    //     let signals = Signals::new(&[SIGINT]).unwrap();
    //     for sig in signals.forever() {
    //         println!("\r\x1b[2KReceived signal {:?}", sig);
    //         signal_tx.send(true).unwrap();
    //         stream_handler.shutdown(Shutdown::Both).unwrap();
    //     }
    // });
    let fut_recv = receiver.begin().fuse();
    let fut_conn = connector.begin().fuse();
    let fut_pool = client_pool.begin().fuse();

    join!(fut_recv, fut_conn, fut_pool);
}

fn get_args() -> Args {
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

    Args {
        bib_chip_file_path: matches.value_of("bibchip").map(|s| s.to_string()),
        participants_file_path: matches.value_of("participants").map(|s| s.to_string()),
        reader_ip: reader,
        reader_port: reader_port,
        bind_port,
        out_file: matches.value_of("file").map(|s| s.to_string()),
        buffered_output: matches.is_present("is_buffered"),
    }
}
