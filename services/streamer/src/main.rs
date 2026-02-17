use clap::{Arg, ArgAction, Command};
use std::convert::TryInto;
use std::net::SocketAddrV4;
use streamer::{ReadType, StreamerConfig};
use tracing::info;

fn validate_socket_addr(value: &str) -> Result<SocketAddrV4, String> {
    streamer::is_socket_addr(value.to_owned())?;
    value
        .parse::<SocketAddrV4>()
        .map_err(|_| "Invalid Socket Address".to_owned())
}

fn validate_port_value(value: &str) -> Result<u16, String> {
    streamer::is_port(value.to_owned())?;
    value
        .parse::<u16>()
        .map_err(|_| "Invalid port number".to_owned())
}

fn validate_read_type(value: &str) -> Result<ReadType, String> {
    value.try_into().map_err(|_| "Invalid read type".to_owned())
}

fn validate_empty_path_value(value: &str) -> Result<String, String> {
    streamer::is_empty_path(value.to_owned())?;
    Ok(value.to_owned())
}

fn validate_existing_file(value: &str) -> Result<String, String> {
    streamer::is_file(value.to_owned())?;
    Ok(value.to_owned())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "streamer starting");

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

    let config = StreamerConfig {
        bib_chip_file_path: matches.get_one::<String>("bibchip").cloned(),
        participants_file_path: matches.get_one::<String>("participants").cloned(),
        readers,
        bind_port: *matches.get_one::<u16>("port").expect("port has a default"),
        out_file: matches.get_one::<String>("file").cloned(),
        buffered_output: matches.get_flag("is_buffered"),
        read_type: *matches
            .get_one::<ReadType>("read_type")
            .expect("read_type has a default value"),
    };

    streamer::run(config).await;
}
