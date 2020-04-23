#[macro_use]
extern crate clap;

use clap::{App, Arg};
use futures::{future::select_all, future::Future, future::FutureExt, pin_mut};
use rusqlite::types::ToSql;
use rusqlite::{Connection, NO_PARAMS};
use std::net::SocketAddrV4;
use std::pin::Pin;
use tokio::signal;
use tokio::sync::mpsc;

mod models;
mod util;
mod workers;
use util::io::{read_bibchip_file, read_participant_file};
use models::Message;
use util::*;
use workers::{ClientConnector, ClientPool, ReaderPool};

async fn signal_handler() {
    signal::ctrl_c().await.unwrap();
}

struct Args {
    bib_chip_file_path: Option<String>,
    participants_file_path: Option<String>,
    readers: Vec<SocketAddrV4>,
    bind_port: u16,
    out_file: Option<String>,
    buffered_output: bool,
}

fn get_args() -> Args {
    // Create the flags
    let matches = App::new("Rusty Timer: Read Streamer")
        .version(crate_version!())
        .author("Isaac Wismer")
        .about("A read streamer for timers")
        .arg(
            Arg::with_name("reader")
                .help("The socket address of the reader to connect to. Eg. 192.168.0.52:10000")
                .index(1)
                .takes_value(true)
                .value_name("reader_ip")
                .validator(is_socket_addr)
                .multiple(true)
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
    let readers: Vec<SocketAddrV4> = matches
        .values_of("reader")
        .unwrap()
        .map(|a| a.parse::<SocketAddrV4>().unwrap())
        .collect();
    // parse the port value
    let bind_port = matches.value_of("port").unwrap().parse::<u16>().unwrap();

    Args {
        bib_chip_file_path: matches.value_of("bibchip").map(|s| s.to_string()),
        participants_file_path: matches.value_of("participants").map(|s| s.to_string()),
        readers: readers,
        bind_port,
        out_file: matches.value_of("file").map(|s| s.to_string()),
        buffered_output: matches.is_present("is_buffered"),
    }
}

#[tokio::main]
async fn main() {
    let args = get_args();

    // Create in memory DB for storing participant data
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

    // Get bib chips
    if args.bib_chip_file_path.is_some() {
        let bib_chips = read_bibchip_file(args.bib_chip_file_path.unwrap().to_string()).unwrap_or_else(|_|vec![]);
        for c in &bib_chips {
            conn.execute(
                "INSERT INTO chip (id, bib)
                        VALUES (?1, ?2)",
                &[&c.id as &dyn ToSql, &c.bib],
            )
            .unwrap();
        }
    }
    // Get participants
    if args.participants_file_path.is_some() {
        let participants = read_participant_file(args.participants_file_path.unwrap().to_string()).unwrap_or_else(|_|vec![]);
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

    // Bus to send messages to client pool
    let (bus_tx, rx) = mpsc::channel::<Message>(1000);

    let client_pool = ClientPool::new(rx, conn, args.out_file, args.buffered_output);
    let connector = ClientConnector::new(args.bind_port, bus_tx.clone()).await;
    let mut reader_pool = ReaderPool::new(args.readers, bus_tx.clone());

    let fut_readers = reader_pool.begin().fuse();
    let fut_clients = client_pool.begin().fuse();
    let fut_conn = connector.begin().fuse();
    let fut_sig = signal_handler().fuse();

    pin_mut!(fut_readers, fut_clients, fut_conn, fut_sig);
    let futures: Vec<Pin<&mut dyn Future<Output = ()>>> = vec![fut_readers, fut_clients, fut_conn, fut_sig];
    select_all(futures).await;
    // If any of them finish, end the program as something went wrong
    bus_tx.clone().send(Message::SHUTDOWN).await.unwrap();
}
