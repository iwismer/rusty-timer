use receiver::headless::{HeadlessConfig, HeadlessHost};
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("receiver-headless: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), String> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = parse_args(std::env::args_os().skip(1))?;
    let host = HeadlessHost::start(config).await?;
    println!(
        "receiver-headless listening on http://{}",
        host.local_addr()
    );

    tokio::signal::ctrl_c()
        .await
        .map_err(|e| format!("failed to listen for shutdown signal: {e}"))?;
    host.shutdown().await
}

fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<HeadlessConfig, String> {
    let mut data_dir: Option<PathBuf> = None;
    let mut bind_addr: SocketAddr = "127.0.0.1:0"
        .parse()
        .expect("default bind address must parse");
    let mut receiver_id: Option<String> = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--data-dir" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--data-dir requires a path".to_owned())?;
                data_dir = Some(PathBuf::from(value));
            }
            "--bind-addr" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--bind-addr requires an address".to_owned())?;
                bind_addr = value
                    .to_string_lossy()
                    .parse()
                    .map_err(|e| format!("invalid --bind-addr: {e}"))?;
                if !bind_addr.ip().is_loopback() {
                    return Err("--bind-addr must be a loopback address".to_owned());
                }
            }
            "--receiver-id" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--receiver-id requires a value".to_owned())?;
                receiver_id = Some(value.to_string_lossy().into_owned());
            }
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("unknown argument: {other}\n{}", usage())),
        }
    }

    Ok(HeadlessConfig {
        data_dir: data_dir.ok_or_else(|| format!("--data-dir is required\n{}", usage()))?,
        bind_addr,
        receiver_id,
    })
}

fn usage() -> String {
    "usage: receiver-headless --data-dir <path> [--bind-addr <addr:port>] [--receiver-id <id>]"
        .to_owned()
}
