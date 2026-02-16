use clap::{Arg, ArgAction, Command};
use futures::{future::select_all, future::FutureExt, pin_mut};
use rusqlite::types::ToSql;
use rusqlite::Connection;
use std::convert::TryInto;
use std::future::Future;
use std::net::SocketAddrV4;
use std::pin::Pin;
use tokio::sync::mpsc;

mod models;
mod util;
mod workers;
use models::{Message, ReadType};
use util::io::{read_bibchip_file, read_participant_file};
use util::*;
use workers::{ClientConnector, ClientPool, ReaderPool};

struct Args {
    bib_chip_file_path: Option<String>,
    participants_file_path: Option<String>,
    readers: Vec<SocketAddrV4>,
    bind_port: u16,
    out_file: Option<String>,
    buffered_output: bool,
    read_type: ReadType,
}

fn validate_socket_addr(value: &str) -> Result<SocketAddrV4, String> {
    is_socket_addr(value.to_owned())?;
    value
        .parse::<SocketAddrV4>()
        .map_err(|_| "Invalid Socket Address".to_owned())
}

fn validate_port_value(value: &str) -> Result<u16, String> {
    is_port(value.to_owned())?;
    value
        .parse::<u16>()
        .map_err(|_| "Invalid port number".to_owned())
}

fn validate_read_type(value: &str) -> Result<ReadType, String> {
    value.try_into().map_err(|_| "Invalid read type".to_owned())
}

fn validate_empty_path_value(value: &str) -> Result<String, String> {
    is_empty_path(value.to_owned())?;
    Ok(value.to_owned())
}

fn validate_existing_file(value: &str) -> Result<String, String> {
    is_file(value.to_owned())?;
    Ok(value.to_owned())
}

fn get_args() -> Args {
    let matches = Command::new("Rusty Timer: Read Streamer")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Isaac Wismer")
        .about("A read streamer for timers")
        .arg(
            Arg::new("reader")
                .help("The socket address of the reader to connect to. Eg. 192.168.0.52:10000")
                .value_name("reader_ip")
                .value_parser(validate_socket_addr)
                .num_args(1..)
                .required(true),
        )
        .arg(
            Arg::new("port")
                .help("The port of the local machine to bind to")
                .short('p')
                .long("port")
                .value_parser(validate_port_value)
                .default_value("10001"),
        )
        .arg(
            Arg::new("read_type")
                .help("The type of read the reader is sending")
                .short('t')
                .long("type")
                .value_parser(validate_read_type)
                .default_value("raw"),
        )
        .arg(
            Arg::new("file")
                .help("The file to output the reads to")
                .short('f')
                .long("file")
                .value_parser(validate_empty_path_value),
        )
        .arg(
            Arg::new("bibchip")
                .help("The bib-chip file")
                .short('b')
                .long("bibchip")
                .value_parser(validate_existing_file),
        )
        .arg(
            Arg::new("participants")
                .help("The .ppl participant file")
                .short('P')
                .long("ppl")
                .value_parser(validate_existing_file)
                .requires("bibchip"),
        )
        .arg(
            Arg::new("is_buffered")
                .help("Buffer the output. Use if high CPU use in encountered")
                .short('B')
                .long("buffer")
                .action(ArgAction::SetTrue),
        )
        .get_matches();

    let readers = matches
        .get_many::<SocketAddrV4>("reader")
        .expect("reader is required")
        .copied()
        .collect();
    let bind_port = *matches
        .get_one::<u16>("port")
        .expect("port has a default value");

    Args {
        bib_chip_file_path: matches.get_one::<String>("bibchip").cloned(),
        participants_file_path: matches.get_one::<String>("participants").cloned(),
        readers,
        bind_port,
        out_file: matches.get_one::<String>("file").cloned(),
        buffered_output: matches.get_flag("is_buffered"),
        read_type: *matches
            .get_one::<ReadType>("read_type")
            .expect("read_type has a default value"),
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
        [],
    )
    .unwrap();

    conn.execute(
        "CREATE TABLE chip (
                  id     TEXT PRIMARY KEY,
                  bib    INTEGER NOT NULL
                  )",
        [],
    )
    .unwrap();

    // Get bib chips
    if args.bib_chip_file_path.is_some() {
        let bib_chips = read_bibchip_file(args.bib_chip_file_path.as_deref().unwrap())
            .unwrap_or_else(|_| vec![]);
        for c in &bib_chips {
            conn.execute(
                "INSERT INTO chip (id, bib)
                        VALUES (?1, ?2)",
                [&c.id as &dyn ToSql, &c.bib],
            )
            .unwrap();
        }
    }
    // Get participants
    if args.participants_file_path.is_some() {
        let participants = read_participant_file(args.participants_file_path.as_deref().unwrap())
            .unwrap_or_else(|_| vec![]);
        for p in &participants {
            conn.execute(
                "INSERT INTO participant (bib, first_name, last_name, gender, affiliation, division)
                        VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                [
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

    let client_pool = ClientPool::new(rx, Some(conn), args.out_file, args.buffered_output);
    let connector = ClientConnector::new(args.bind_port, bus_tx.clone()).await;
    let mut reader_pool = ReaderPool::new(args.readers, bus_tx.clone(), args.read_type);

    let fut_readers = reader_pool.begin().fuse();
    let fut_clients = client_pool.begin().fuse();
    let fut_conn = connector.begin().fuse();
    let fut_sig = signal_handler().fuse();

    pin_mut!(fut_readers, fut_clients, fut_conn, fut_sig);
    let futures: Vec<Pin<&mut dyn Future<Output = ()>>> =
        vec![fut_readers, fut_clients, fut_conn, fut_sig];
    select_all(futures).await;
    // If any of them finish, end the program as something went wrong
    bus_tx.send(Message::SHUTDOWN).await.unwrap();
}
