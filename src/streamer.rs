#[macro_use]
extern crate clap;

use clap::{App, Arg};
use futures::{future::FutureExt, pin_mut, select};
use rusqlite::types::ToSql;
use rusqlite::{Connection, NO_PARAMS};
use std::net::Ipv4Addr;
use tokio::sync::mpsc;
use tokio::signal;

mod models;
mod util;
mod workers;
use models::chip::read_bibchip_file;
use models::participant::read_participant_file;
use models::Message;
use workers::{ClientConnector, ClientPool, ReadBroadcaster};
use util::*;

async fn signal_handler() {
    signal::ctrl_c().await.unwrap();
}

struct Args {
    bib_chip_file_path: Option<String>,
    participants_file_path: Option<String>,
    reader_ip: Ipv4Addr,
    reader_port: u16,
    bind_port: u16,
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
    let (bus_tx, rx) = mpsc::channel::<Message>(10);

    let client_pool = ClientPool::new(rx);
    let connector = ClientConnector::new(args.bind_port, bus_tx.clone()).await;
    let receiver = ReadBroadcaster::new(
        args.reader_ip,
        args.reader_port,
        bus_tx.clone(),
        conn,
        args.out_file,
        args.buffered_output,
    )
    .await;

    let fut_recv = receiver.begin().fuse();
    let fut_conn = connector.begin().fuse();
    let fut_pool = client_pool.begin().fuse();
    let fut_sig = signal_handler().fuse();
    pin_mut!(fut_recv, fut_conn, fut_pool, fut_sig);
    select! {
        () = fut_recv => println!("Receiver crashed"),
        () = fut_conn => println!("Client connector crashed"),
        () = fut_pool => println!("Client pool crashed"),
        () = fut_sig => println!("Signal received"),
    };
    bus_tx.clone().send(Message::SHUTDOWN).await.unwrap();
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
    let bind_port = matches.value_of("port").unwrap().parse::<u16>().unwrap();
    let reader_port = matches
        .value_of("reader-port")
        .unwrap()
        .parse::<u16>()
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
