use receiver::headless::{HeadlessConfig, HeadlessHost};
use receiver::p2p_runtime::{
    ForwarderPeerConfig, MIN_RECONCILE_INTERVAL, P2pReceiverConfig, ReceiverIdentity,
    ServerClientConfig, node_id_for_seed, parse_secret_key_seed_hex,
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
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    // `print-node-id` is a non-binding helper subcommand: it derives the
    // deterministic loopback node id for a seed and exits without binding any
    // endpoint or initializing logging (so stdout carries only the id). The
    // forwarder and receiver share the same seed->id derivation, so the E2E
    // stack orchestrator uses this to learn both peers' ids before startup.
    if let Some(seed_hex) = node_id_subcommand(&args)? {
        let seed = parse_secret_key_seed_hex(&seed_hex)?;
        println!("{}", node_id_for_seed(seed));
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = parse_args(args)?;
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

/// If the first CLI argument is the `print-node-id` subcommand, returns the
/// secret-key seed hex that followed `--p2p-secret-key-seed-hex`. Returns `None`
/// for the normal (server) invocation so the caller falls through to
/// [`parse_args`].
fn node_id_subcommand(args: &[OsString]) -> Result<Option<String>, String> {
    let mut iter = args.iter();
    match iter.next() {
        Some(first) if first == "print-node-id" => {}
        _ => return Ok(None),
    }
    let mut seed_hex: Option<String> = None;
    while let Some(arg) = iter.next() {
        match arg.to_string_lossy().as_ref() {
            "--p2p-secret-key-seed-hex" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--p2p-secret-key-seed-hex requires a value".to_owned())?;
                seed_hex = Some(value.to_string_lossy().into_owned());
            }
            other => return Err(format!("unknown argument for print-node-id: {other}")),
        }
    }
    seed_hex
        .map(Some)
        .ok_or_else(|| "print-node-id requires --p2p-secret-key-seed-hex <64-hex>".to_owned())
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
    let mut secret_key_path: Option<PathBuf> = None;
    let mut relay_disabled = false;
    let mut discovery_disabled = false;
    let mut server_url: Option<String> = None;
    let mut server_token: Option<String> = None;
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
            "--p2p-secret-key-path" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-secret-key-path requires a path".to_owned())?;
                secret_key_path = Some(PathBuf::from(value));
            }
            "--p2p-relay-disabled" => {
                relay_disabled = true;
            }
            "--p2p-discovery-disabled" => {
                discovery_disabled = true;
            }
            "--p2p-server-url" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-server-url requires a URL".to_owned())?;
                server_url = Some(value.to_string_lossy().into_owned());
            }
            "--p2p-server-token" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-server-token requires a value".to_owned())?;
                server_token = Some(value.to_string_lossy().into_owned());
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

    let data_dir = data_dir.ok_or_else(|| format!("--data-dir is required\n{}", usage()))?;
    let default_key_path = data_dir.join("p2p_secret.key");
    let p2p = build_p2p_config(
        forwarder_node_id,
        forwarder_direct_addr,
        secret_key_seed,
        secret_key_path,
        relay_disabled,
        discovery_disabled,
        server_url,
        server_token,
        reconcile_ms,
        default_key_path,
    )?;

    Ok(HeadlessConfig {
        data_dir,
        bind_addr,
        receiver_id,
        p2p,
    })
}

/// Assemble the optional P2P config from parsed flags. P2P is enabled only when
/// at least one P2P flag is present; the forwarder node id and direct address
/// must be supplied together (both or neither); the server URL and token must
/// be supplied together; and at least one of an explicit forwarder or a server
/// must be configured.
///
/// Identity: a seed and an explicit key path are mutually exclusive; when
/// neither is given, a persistent key at `default_key_path` is used. A seed
/// implies the loopback/dev transport (relays + discovery off, loopback bind);
/// a key path uses production transport unless the disable flags are set.
#[allow(clippy::too_many_arguments)]
fn build_p2p_config(
    forwarder_node_id: Option<String>,
    forwarder_direct_addr: Option<SocketAddr>,
    secret_key_seed: Option<[u8; 32]>,
    secret_key_path: Option<PathBuf>,
    relay_disabled: bool,
    discovery_disabled: bool,
    server_url: Option<String>,
    server_token: Option<String>,
    reconcile_ms: Option<u64>,
    default_key_path: PathBuf,
) -> Result<Option<P2pReceiverConfig>, String> {
    let any_p2p = forwarder_node_id.is_some()
        || forwarder_direct_addr.is_some()
        || secret_key_seed.is_some()
        || secret_key_path.is_some()
        || relay_disabled
        || discovery_disabled
        || server_url.is_some()
        || server_token.is_some()
        || reconcile_ms.is_some();
    if !any_p2p {
        return Ok(None);
    }

    let forwarder = match (forwarder_node_id, forwarder_direct_addr) {
        (Some(node_id), Some(direct_addr)) => Some(ForwarderPeerConfig {
            node_id,
            direct_addr,
        }),
        (None, None) => None,
        (Some(_), None) => {
            return Err(format!(
                "--p2p-forwarder-direct-addr is required when --p2p-forwarder-node-id is set\n{}",
                usage()
            ));
        }
        (None, Some(_)) => {
            return Err(format!(
                "--p2p-forwarder-node-id is required when --p2p-forwarder-direct-addr is set\n{}",
                usage()
            ));
        }
    };

    let server = match (server_url, server_token) {
        (Some(url), Some(token)) => Some(ServerClientConfig { url, token }),
        (None, None) => None,
        _ => {
            return Err(format!(
                "--p2p-server-url and --p2p-server-token must be set together\n{}",
                usage()
            ));
        }
    };

    if forwarder.is_none() && server.is_none() {
        return Err(format!(
            "P2P requires either an explicit forwarder (--p2p-forwarder-node-id + \
             --p2p-forwarder-direct-addr) or a server (--p2p-server-url + \
             --p2p-server-token)\n{}",
            usage()
        ));
    }

    let identity = match (secret_key_seed, secret_key_path) {
        (Some(_), Some(_)) => {
            return Err(format!(
                "--p2p-secret-key-seed-hex and --p2p-secret-key-path are mutually exclusive\n{}",
                usage()
            ));
        }
        (Some(seed), None) => ReceiverIdentity::Seed(seed),
        (None, Some(path)) => ReceiverIdentity::KeyPath(path),
        (None, None) => ReceiverIdentity::KeyPath(default_key_path),
    };
    // A seed is loopback/dev: force relays + discovery off and bind loopback.
    let seed_identity = matches!(identity, ReceiverIdentity::Seed(_));
    let relay_disabled = relay_disabled || seed_identity;
    let discovery_disabled = discovery_disabled || seed_identity;
    let bind_addr_v4 =
        seed_identity.then(|| std::net::SocketAddrV4::new(std::net::Ipv4Addr::LOCALHOST, 0));

    let reconcile_interval = Duration::from_millis(reconcile_ms.unwrap_or(1000));
    if reconcile_interval < MIN_RECONCILE_INTERVAL {
        return Err(format!(
            "--p2p-reconcile-ms must be at least {} ms\n{}",
            MIN_RECONCILE_INTERVAL.as_millis(),
            usage()
        ));
    }

    Ok(Some(P2pReceiverConfig {
        identity,
        relay_disabled,
        discovery_disabled,
        bind_addr_v4,
        forwarder,
        server,
        reconcile_interval,
    }))
}

fn usage() -> String {
    concat!(
        "usage: receiver-headless --data-dir <path> [--bind-addr <addr:port>] [--receiver-id <id>]\n",
        "\n",
        "P2P (configure an explicit forwarder, a server, or both):\n",
        "  [--p2p-secret-key-seed-hex <64-hex>]  (dev/loopback identity; default: persistent key in data dir)\n",
        "  [--p2p-secret-key-path <path>]  (persistent identity; mutually exclusive with seed)\n",
        "  [--p2p-relay-disabled] [--p2p-discovery-disabled]  (transport; forced on for a seed)\n",
        "  [--p2p-forwarder-node-id <node-id> --p2p-forwarder-direct-addr <ip:port>]\n",
        "  [--p2p-server-url <url> --p2p-server-token <token>]\n",
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
    fn print_node_id_subcommand_returns_seed_hex() {
        let got = node_id_subcommand(&args(&[
            "print-node-id",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
        ]))
        .unwrap();
        assert_eq!(got.as_deref(), Some(SEED_HEX));
    }

    #[test]
    fn no_subcommand_returns_none() {
        let got = node_id_subcommand(&args(&["--data-dir", "/tmp/x"])).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn print_node_id_subcommand_requires_seed() {
        let err = node_id_subcommand(&args(&["print-node-id"])).unwrap_err();
        assert!(err.contains("requires --p2p-secret-key-seed-hex"));
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
        let fwd = p2p.forwarder.as_ref().expect("forwarder present");
        assert_eq!(fwd.node_id, node_id);
        assert_eq!(fwd.direct_addr, "127.0.0.1:5000".parse().unwrap());
        assert!(matches!(p2p.identity, ReceiverIdentity::Seed(s) if s == [0xab; 32]));
        assert_eq!(p2p.reconcile_interval, Duration::from_millis(50));
        assert!(p2p.server.is_none());
    }

    #[test]
    fn server_only_flags_parse_without_forwarder() {
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
            "--p2p-server-url",
            "http://127.0.0.1:8080",
            "--p2p-server-token",
            "secret-token",
        ]))
        .unwrap();
        let p2p = config.p2p.expect("p2p config present");
        assert!(
            p2p.forwarder.is_none(),
            "server-only config must not require an explicit forwarder"
        );
        let thin = p2p.server.expect("server config");
        assert_eq!(thin.url, "http://127.0.0.1:8080");
    }

    #[test]
    fn p2p_requires_forwarder_or_server() {
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
        ]))
        .unwrap_err();
        assert!(err.contains("either an explicit forwarder"), "got: {err}");
    }

    #[test]
    fn full_p2p_flags_with_server_parse() {
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
            "--p2p-server-url",
            "http://127.0.0.1:8080",
            "--p2p-server-token",
            "secret-token",
        ]))
        .unwrap();
        let thin = config.p2p.unwrap().server.expect("server config");
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
    fn partial_server_rejected() {
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
            "--p2p-server-url",
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

    #[test]
    fn server_only_without_seed_uses_default_key_path() {
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-server-url",
            "http://127.0.0.1:8080",
            "--p2p-server-token",
            "secret-token",
        ]))
        .unwrap();
        let p2p = config.p2p.expect("p2p config present");
        match p2p.identity {
            ReceiverIdentity::KeyPath(path) => {
                assert_eq!(path, PathBuf::from("/tmp/x").join("p2p_secret.key"));
            }
            other => panic!("expected KeyPath identity, got {other:?}"),
        }
        // Key-path identity defaults to production transport.
        assert!(!p2p.relay_disabled);
        assert!(!p2p.discovery_disabled);
        assert!(p2p.bind_addr_v4.is_none());
    }

    #[test]
    fn seed_and_key_path_mutually_exclusive() {
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-server-url",
            "http://127.0.0.1:8080",
            "--p2p-server-token",
            "secret-token",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
            "--p2p-secret-key-path",
            "/tmp/x/key",
        ]))
        .unwrap_err();
        assert!(err.contains("mutually exclusive"), "got: {err}");
    }
}
