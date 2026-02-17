use clap::{Arg, Command};
use emulator::{EmulatorConfig, ReadType};
use std::convert::TryFrom;
use tracing::info;

fn validate_port_value(value: &str) -> Result<u16, String> {
    emulator::is_port(value.to_owned())?;
    value
        .parse::<u16>()
        .map_err(|_| "Invalid port number".to_owned())
}

fn validate_delay_value(value: &str) -> Result<u64, String> {
    emulator::is_delay(value.to_owned())?;
    value
        .parse::<u64>()
        .map_err(|_| "Invalid delay value".to_owned())
}

fn validate_file_value(value: &str) -> Result<String, String> {
    emulator::is_file(value.to_owned())?;
    Ok(value.to_owned())
}

fn validate_read_type(value: &str) -> Result<ReadType, String> {
    ReadType::try_from(value).map_err(|_| "Invalid read type".to_owned())
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "emulator starting");

    let matches = Command::new("Read Emulator")
        .version(env!("CARGO_PKG_VERSION"))
        .author("Isaac Wismer")
        .about("A chip read emulator for timers")
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

    let config = EmulatorConfig {
        bind_port: *matches.get_one::<u16>("port").expect("port has a default"),
        delay: *matches
            .get_one::<u64>("delay")
            .expect("delay has a default"),
        file_path: matches.get_one::<String>("file").cloned(),
        read_type: *matches
            .get_one::<ReadType>("read_type")
            .expect("read_type has a default"),
    };

    emulator::run(config).await;
}
