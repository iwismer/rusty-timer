use receiver::headless::{HeadlessConfig, HeadlessHost};
use receiver::p2p_runtime::{
    ForwarderPeerConfig, MIN_RECONCILE_INTERVAL, P2pReceiverConfig, ThinNodeClientConfig,
    parse_secret_key_seed_hex,
};
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

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
    let mut forwarder_node_id: Option<String> = None;
    let mut forwarder_direct_addr: Option<SocketAddr> = None;
    let mut secret_key_seed: Option<[u8; 32]> = None;
    let mut thin_node_url: Option<String> = None;
    let mut thin_node_token: Option<String> = None;
    let mut reconcile_ms: Option<u64> = None;
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
            "--p2p-forwarder-node-id" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-forwarder-node-id requires a value".to_owned())?;
                forwarder_node_id = Some(value.to_string_lossy().into_owned());
            }
            "--p2p-forwarder-direct-addr" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-forwarder-direct-addr requires an address".to_owned())?;
                forwarder_direct_addr = Some(
                    value
                        .to_string_lossy()
                        .parse()
                        .map_err(|e| format!("invalid --p2p-forwarder-direct-addr: {e}"))?,
                );
            }
            "--p2p-secret-key-seed-hex" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-secret-key-seed-hex requires a value".to_owned())?;
                secret_key_seed = Some(parse_secret_key_seed_hex(&value.to_string_lossy())?);
            }
            "--p2p-thin-node-url" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-thin-node-url requires a URL".to_owned())?;
                thin_node_url = Some(value.to_string_lossy().into_owned());
            }
            "--p2p-thin-node-token" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-thin-node-token requires a value".to_owned())?;
                thin_node_token = Some(value.to_string_lossy().into_owned());
            }
            "--p2p-reconcile-ms" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-reconcile-ms requires a value".to_owned())?;
                reconcile_ms = Some(
                    value
                        .to_string_lossy()
                        .parse()
                        .map_err(|e| format!("invalid --p2p-reconcile-ms: {e}"))?,
                );
            }
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("unknown argument: {other}\n{}", usage())),
        }
    }

    let p2p = build_p2p_config(
        forwarder_node_id,
        forwarder_direct_addr,
        secret_key_seed,
        thin_node_url,
        thin_node_token,
        reconcile_ms,
    )?;

    Ok(HeadlessConfig {
        data_dir: data_dir.ok_or_else(|| format!("--data-dir is required\n{}", usage()))?,
        bind_addr,
        receiver_id,
        p2p,
    })
}

/// Assemble the optional P2P config from parsed flags. P2P is enabled only when
/// at least one P2P flag is present; partial config is rejected with a clear
/// message. The thin-node URL and token must be supplied together.
fn build_p2p_config(
    forwarder_node_id: Option<String>,
    forwarder_direct_addr: Option<SocketAddr>,
    secret_key_seed: Option<[u8; 32]>,
    thin_node_url: Option<String>,
    thin_node_token: Option<String>,
    reconcile_ms: Option<u64>,
) -> Result<Option<P2pReceiverConfig>, String> {
    let any_p2p = forwarder_node_id.is_some()
        || forwarder_direct_addr.is_some()
        || secret_key_seed.is_some()
        || thin_node_url.is_some()
        || thin_node_token.is_some()
        || reconcile_ms.is_some();
    if !any_p2p {
        return Ok(None);
    }

    let forwarder_node_id = forwarder_node_id.ok_or_else(|| {
        format!(
            "--p2p-forwarder-node-id is required when P2P flags are set\n{}",
            usage()
        )
    })?;
    let direct_addr = forwarder_direct_addr.ok_or_else(|| {
        format!(
            "--p2p-forwarder-direct-addr is required when P2P flags are set\n{}",
            usage()
        )
    })?;
    let secret_key_seed = secret_key_seed.ok_or_else(|| {
        format!(
            "--p2p-secret-key-seed-hex is required when P2P flags are set\n{}",
            usage()
        )
    })?;

    let thin_node = match (thin_node_url, thin_node_token) {
        (Some(url), Some(token)) => Some(ThinNodeClientConfig { url, token }),
        (None, None) => None,
        _ => {
            return Err(format!(
                "--p2p-thin-node-url and --p2p-thin-node-token must be set together\n{}",
                usage()
            ));
        }
    };

    let reconcile_interval = Duration::from_millis(reconcile_ms.unwrap_or(1000));
    if reconcile_interval < MIN_RECONCILE_INTERVAL {
        return Err(format!(
            "--p2p-reconcile-ms must be at least {} ms\n{}",
            MIN_RECONCILE_INTERVAL.as_millis(),
            usage()
        ));
    }

    Ok(Some(P2pReceiverConfig {
        secret_key_seed,
        forwarder: ForwarderPeerConfig {
            node_id: forwarder_node_id,
            direct_addr,
        },
        thin_node,
        reconcile_interval,
    }))
}

fn usage() -> String {
    concat!(
        "usage: receiver-headless --data-dir <path> [--bind-addr <addr:port>] [--receiver-id <id>]\n",
        "\n",
        "P2P (forwarder-node-id, forwarder-direct-addr, secret-key-seed-hex required together):\n",
        "  --p2p-forwarder-node-id <node-id>\n",
        "  --p2p-forwarder-direct-addr <ip:port>\n",
        "  --p2p-secret-key-seed-hex <64-hex>\n",
        "  [--p2p-thin-node-url <url> --p2p-thin-node-token <token>]\n",
        "  [--p2p-reconcile-ms <ms>]  (must be >= 50)",
    )
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    const SEED_HEX: &str = "abababababababababababababababababababababababababababababababab";
    // A valid deterministic loopback node id (public key for seed [0xcd; 32]).
    fn forwarder_node_id() -> String {
        receiver::p2p_runtime::node_id_for_seed([0xcd; 32])
    }

    #[test]
    fn no_p2p_flags_preserves_old_config() {
        let config = parse_args(args(&["--data-dir", "/tmp/x"])).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/tmp/x"));
        assert!(config.p2p.is_none(), "no p2p flags must leave p2p disabled");
    }

    #[test]
    fn full_p2p_flags_parse() {
        let node_id = forwarder_node_id();
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-node-id",
            &node_id,
            "--p2p-forwarder-direct-addr",
            "127.0.0.1:5000",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
            "--p2p-reconcile-ms",
            "50",
        ]))
        .unwrap();
        let p2p = config.p2p.expect("p2p config present");
        assert_eq!(p2p.forwarder.node_id, node_id);
        assert_eq!(p2p.forwarder.direct_addr, "127.0.0.1:5000".parse().unwrap());
        assert_eq!(p2p.secret_key_seed, [0xab; 32]);
        assert_eq!(p2p.reconcile_interval, Duration::from_millis(50));
        assert!(p2p.thin_node.is_none());
    }

    #[test]
    fn full_p2p_flags_with_thin_node_parse() {
        let node_id = forwarder_node_id();
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-node-id",
            &node_id,
            "--p2p-forwarder-direct-addr",
            "127.0.0.1:5000",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
            "--p2p-thin-node-url",
            "http://127.0.0.1:8080",
            "--p2p-thin-node-token",
            "secret-token",
        ]))
        .unwrap();
        let thin = config.p2p.unwrap().thin_node.expect("thin node config");
        assert_eq!(thin.url, "http://127.0.0.1:8080");
        assert_eq!(thin.token, "secret-token");
    }

    #[test]
    fn partial_p2p_flags_rejected() {
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-node-id",
            "some-id",
        ]))
        .unwrap_err();
        assert!(
            err.contains("--p2p-forwarder-direct-addr is required"),
            "got: {err}"
        );
    }

    #[test]
    fn partial_thin_node_rejected() {
        let node_id = forwarder_node_id();
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-node-id",
            &node_id,
            "--p2p-forwarder-direct-addr",
            "127.0.0.1:5000",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
            "--p2p-thin-node-url",
            "http://127.0.0.1:8080",
        ]))
        .unwrap_err();
        assert!(err.contains("must be set together"), "got: {err}");
    }

    #[test]
    fn zero_reconcile_ms_rejected() {
        let node_id = forwarder_node_id();
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-node-id",
            &node_id,
            "--p2p-forwarder-direct-addr",
            "127.0.0.1:5000",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
            "--p2p-reconcile-ms",
            "0",
        ]))
        .unwrap_err();
        assert!(err.contains("at least 50 ms"), "got: {err}");
    }

    #[test]
    fn tiny_reconcile_ms_rejected() {
        let node_id = forwarder_node_id();
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-node-id",
            &node_id,
            "--p2p-forwarder-direct-addr",
            "127.0.0.1:5000",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
            "--p2p-reconcile-ms",
            "10",
        ]))
        .unwrap_err();
        assert!(err.contains("at least 50 ms"), "got: {err}");
    }

    #[test]
    fn minimum_reconcile_ms_accepted() {
        let node_id = forwarder_node_id();
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-node-id",
            &node_id,
            "--p2p-forwarder-direct-addr",
            "127.0.0.1:5000",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
            "--p2p-reconcile-ms",
            "50",
        ]))
        .unwrap();
        assert_eq!(
            config.p2p.unwrap().reconcile_interval,
            Duration::from_millis(50)
        );
    }

    #[test]
    fn bad_seed_hex_rejected() {
        let node_id = forwarder_node_id();
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-node-id",
            &node_id,
            "--p2p-forwarder-direct-addr",
            "127.0.0.1:5000",
            "--p2p-secret-key-seed-hex",
            "deadbeef",
        ]))
        .unwrap_err();
        assert!(err.contains("64 hex characters"), "got: {err}");
    }
}
