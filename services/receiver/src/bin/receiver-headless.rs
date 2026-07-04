use receiver::headless::{HeadlessConfig, HeadlessHost};
use receiver::p2p_runtime::{
    ENV_P2P_DISCOVERY_DISABLED, ENV_P2P_FORWARDER_DIRECT_ADDR, ENV_P2P_FORWARDER_ENDPOINT_ID,
    ENV_P2P_RECONCILE_MS, ENV_P2P_RELAY_DISABLED, ENV_P2P_SECRET_KEY_PATH,
    ENV_P2P_SECRET_KEY_SEED_HEX, ENV_P2P_SERVER_TOKEN, ENV_P2P_SERVER_URL, endpoint_id_for_seed,
    p2p_config_from_lookup, parse_secret_key_seed_hex,
};
use std::collections::HashMap;
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
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();

    // `print-endpoint-id` is a non-binding helper subcommand: it derives the
    // deterministic loopback endpoint id for a seed and exits without binding any
    // endpoint or initializing logging (so stdout carries only the id). The
    // forwarder and receiver share the same seed->id derivation, so the E2E
    // stack orchestrator uses this to learn both peers' ids before startup.
    if let Some(seed_hex) = endpoint_id_subcommand(&args)? {
        let seed = parse_secret_key_seed_hex(&seed_hex)?;
        println!("{}", endpoint_id_for_seed(seed));
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let config = parse_args(args, |key| std::env::var(key).ok())?;
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

/// If the first CLI argument is the `print-endpoint-id` subcommand, returns the
/// secret-key seed hex that followed `--p2p-secret-key-seed-hex`. Returns `None`
/// for the normal (server) invocation so the caller falls through to
/// [`parse_args`].
fn endpoint_id_subcommand(args: &[OsString]) -> Result<Option<String>, String> {
    let mut iter = args.iter();
    match iter.next() {
        Some(first) if first == "print-endpoint-id" => {}
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
            other => return Err(format!("unknown argument for print-endpoint-id: {other}")),
        }
    }
    seed_hex
        .map(Some)
        .ok_or_else(|| "print-endpoint-id requires --p2p-secret-key-seed-hex <64-hex>".to_owned())
}

/// Parse CLI arguments into a [`HeadlessConfig`].
///
/// The binary keeps only flag parsing and error decoration; every P2P
/// assembly rule (flag pairing, seed-implied loopback defaults, reconcile
/// minimum, default key path, `server_override` recording) lives in
/// [`p2p_config_from_lookup`]. CLI flags are converted into the lookup's
/// key/value format and layered over `env` with precedence (highest first):
/// explicit CLI flag > env value > seed-implied default. The CLI booleans
/// (`--p2p-relay-disabled`, `--p2p-discovery-disabled`) are disable-only and
/// cannot re-enable what a seed default disables; an explicit env value
/// (e.g. `RT_P2P_RELAY_DISABLED=0`) can.
fn parse_args(
    args: impl IntoIterator<Item = OsString>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<HeadlessConfig, String> {
    let mut data_dir: Option<PathBuf> = None;
    let mut bind_addr: SocketAddr = "127.0.0.1:0"
        .parse()
        .expect("default bind address must parse");
    let mut receiver_id: Option<String> = None;
    // CLI-provided P2P values, keyed by the canonical lookup keys shared with
    // the env config path.
    let mut cli: HashMap<&'static str, String> = HashMap::new();
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
            "--p2p-forwarder-endpoint-id" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-forwarder-endpoint-id requires a value".to_owned())?;
                cli.insert(
                    ENV_P2P_FORWARDER_ENDPOINT_ID,
                    value.to_string_lossy().into_owned(),
                );
            }
            "--p2p-forwarder-direct-addr" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-forwarder-direct-addr requires an address".to_owned())?;
                cli.insert(
                    ENV_P2P_FORWARDER_DIRECT_ADDR,
                    value.to_string_lossy().into_owned(),
                );
            }
            "--p2p-secret-key-seed-hex" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-secret-key-seed-hex requires a value".to_owned())?;
                cli.insert(
                    ENV_P2P_SECRET_KEY_SEED_HEX,
                    value.to_string_lossy().into_owned(),
                );
            }
            "--p2p-secret-key-path" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-secret-key-path requires a path".to_owned())?;
                // Known limitation: the path round-trips through
                // `to_string_lossy` here and `trim` in `p2p_config_from_lookup`,
                // so non-UTF-8 paths are lossily converted and
                // whitespace-padded paths are altered. Shared with the
                // RT_P2P_SECRET_KEY_PATH env path, which is a string anyway.
                cli.insert(
                    ENV_P2P_SECRET_KEY_PATH,
                    value.to_string_lossy().into_owned(),
                );
            }
            "--p2p-relay-disabled" => {
                cli.insert(ENV_P2P_RELAY_DISABLED, "1".to_owned());
            }
            "--p2p-discovery-disabled" => {
                cli.insert(ENV_P2P_DISCOVERY_DISABLED, "1".to_owned());
            }
            "--p2p-server-url" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-server-url requires a URL".to_owned())?;
                cli.insert(ENV_P2P_SERVER_URL, value.to_string_lossy().into_owned());
            }
            "--p2p-server-token" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-server-token requires a value".to_owned())?;
                cli.insert(ENV_P2P_SERVER_TOKEN, value.to_string_lossy().into_owned());
            }
            "--p2p-reconcile-ms" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--p2p-reconcile-ms requires a value".to_owned())?;
                cli.insert(ENV_P2P_RECONCILE_MS, value.to_string_lossy().into_owned());
            }
            "--help" | "-h" => return Err(usage()),
            other => return Err(format!("unknown argument: {other}\n{}", usage())),
        }
    }

    let data_dir = data_dir.ok_or_else(|| format!("--data-dir is required\n{}", usage()))?;
    let default_key_path = data_dir.join("p2p_secret.key");
    let p2p = p2p_config_from_lookup(
        |key| cli.get(key).cloned().or_else(|| env(key)),
        default_key_path,
    )
    .map_err(|e| decorate_p2p_error(&e))?;

    Ok(HeadlessConfig {
        data_dir,
        bind_addr,
        receiver_id,
        p2p,
    })
}

/// Rewrite lookup-key names in a [`p2p_config_from_lookup`] error to the CLI
/// flag spellings and append usage, so `receiver-headless` errors speak the
/// binary's own flag language.
fn decorate_p2p_error(err: &str) -> String {
    const KEY_TO_FLAG: &[(&str, &str)] = &[
        (ENV_P2P_FORWARDER_ENDPOINT_ID, "--p2p-forwarder-endpoint-id"),
        (ENV_P2P_FORWARDER_DIRECT_ADDR, "--p2p-forwarder-direct-addr"),
        (ENV_P2P_SECRET_KEY_SEED_HEX, "--p2p-secret-key-seed-hex"),
        (ENV_P2P_SECRET_KEY_PATH, "--p2p-secret-key-path"),
        (ENV_P2P_RELAY_DISABLED, "--p2p-relay-disabled"),
        (ENV_P2P_DISCOVERY_DISABLED, "--p2p-discovery-disabled"),
        (ENV_P2P_SERVER_URL, "--p2p-server-url"),
        (ENV_P2P_SERVER_TOKEN, "--p2p-server-token"),
        (ENV_P2P_RECONCILE_MS, "--p2p-reconcile-ms"),
    ];
    let mut msg = err.to_owned();
    for (key, flag) in KEY_TO_FLAG {
        msg = msg.replace(key, flag);
    }
    format!("{msg}\n{}", usage())
}

fn usage() -> String {
    concat!(
        "usage: receiver-headless --data-dir <path> [--bind-addr <addr:port>] [--receiver-id <id>]\n",
        "\n",
        "P2P (all optional; the stored profile supplies the server when omitted):\n",
        "  [--p2p-secret-key-seed-hex <64-hex>]  (dev/loopback identity; default: persistent key in data dir)\n",
        "  [--p2p-secret-key-path <path>]  (persistent identity; mutually exclusive with seed)\n",
        "  [--p2p-relay-disabled] [--p2p-discovery-disabled]  (transport; forced on for a seed)\n",
        "  [--p2p-forwarder-endpoint-id <endpoint-id> --p2p-forwarder-direct-addr <ip:port>]\n",
        "  [--p2p-server-url <url> --p2p-server-token <token>]\n",
        "  [--p2p-reconcile-ms <ms>]  (must be >= 50)\n",
        "\n",
        "Each P2P flag falls back to its RT_P2P_* environment variable when omitted.",
    )
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use receiver::p2p_runtime::ReceiverIdentity;
    use std::time::Duration;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    /// CLI-only parse: shadows the binary's `parse_args` with an empty env
    /// lookup so tests are hermetic against ambient `RT_P2P_*` variables.
    fn parse_args(args: Vec<OsString>) -> Result<HeadlessConfig, String> {
        super::parse_args(args, |_| None)
    }

    /// Parse with an injected env lookup for precedence tests.
    fn parse_args_with_env(
        args: Vec<OsString>,
        env: &[(&str, &str)],
    ) -> Result<HeadlessConfig, String> {
        let map: HashMap<String, String> = env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        super::parse_args(args, move |key| map.get(key).cloned())
    }

    const SEED_HEX: &str = "abababababababababababababababababababababababababababababababab";
    // A valid deterministic loopback endpoint id (public key for seed [0xcd; 32]).
    fn forwarder_endpoint_id() -> String {
        receiver::p2p_runtime::endpoint_id_for_seed([0xcd; 32])
    }

    #[test]
    fn print_endpoint_id_subcommand_returns_seed_hex() {
        let got = endpoint_id_subcommand(&args(&[
            "print-endpoint-id",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
        ]))
        .unwrap();
        assert_eq!(got.as_deref(), Some(SEED_HEX));
    }

    #[test]
    fn no_subcommand_returns_none() {
        let got = endpoint_id_subcommand(&args(&["--data-dir", "/tmp/x"])).unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn print_endpoint_id_subcommand_requires_seed() {
        let err = endpoint_id_subcommand(&args(&["print-endpoint-id"])).unwrap_err();
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
        let endpoint_id = forwarder_endpoint_id();
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-endpoint-id",
            &endpoint_id,
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
        assert_eq!(fwd.endpoint_id, endpoint_id);
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
    fn p2p_transport_only_parses_without_forwarder_or_server() {
        // Identity/transport flags with no forwarder and no server are valid:
        // `HeadlessHost::start` resolves the server from the stored profile.
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
            "--p2p-relay-disabled",
        ]))
        .unwrap();
        let p2p = config.p2p.expect("p2p config present");
        assert!(p2p.forwarder.is_none());
        assert!(p2p.server.is_none());
        assert!(p2p.relay_disabled);
    }

    #[test]
    fn full_p2p_flags_with_server_parse() {
        let endpoint_id = forwarder_endpoint_id();
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-endpoint-id",
            &endpoint_id,
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
            "--p2p-forwarder-endpoint-id",
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
        let endpoint_id = forwarder_endpoint_id();
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-endpoint-id",
            &endpoint_id,
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
        let endpoint_id = forwarder_endpoint_id();
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-endpoint-id",
            &endpoint_id,
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
        let endpoint_id = forwarder_endpoint_id();
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-endpoint-id",
            &endpoint_id,
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
        let endpoint_id = forwarder_endpoint_id();
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-endpoint-id",
            &endpoint_id,
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
        let endpoint_id = forwarder_endpoint_id();
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-forwarder-endpoint-id",
            &endpoint_id,
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

    #[test]
    fn seed_implies_loopback_transport_defaults() {
        let config = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-secret-key-seed-hex",
            SEED_HEX,
        ]))
        .unwrap();
        let p2p = config.p2p.expect("p2p config present");
        assert!(p2p.relay_disabled, "seed must default relays off");
        assert!(p2p.discovery_disabled, "seed must default discovery off");
        let bind = p2p.bind_addr_v4.expect("seed must default loopback bind");
        assert!(bind.ip().is_loopback());
    }

    #[test]
    fn server_override_recorded_by_lookup_assembly() {
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
        assert_eq!(
            p2p.server_override,
            (
                Some("http://127.0.0.1:8080".to_owned()),
                Some("secret-token".to_owned())
            ),
            "lookup assembly must record the CLI server flags as the override"
        );
    }

    #[test]
    fn cli_flag_overrides_env_value() {
        let config = parse_args_with_env(
            args(&[
                "--data-dir",
                "/tmp/x",
                "--p2p-secret-key-seed-hex",
                SEED_HEX,
                "--p2p-reconcile-ms",
                "250",
            ]),
            &[(ENV_P2P_RECONCILE_MS, "999")],
        )
        .unwrap();
        assert_eq!(
            config.p2p.unwrap().reconcile_interval,
            Duration::from_millis(250),
            "explicit CLI flag must win over the env value"
        );
    }

    #[test]
    fn env_value_applies_when_cli_flag_absent() {
        let config = parse_args_with_env(
            args(&[
                "--data-dir",
                "/tmp/x",
                "--p2p-secret-key-seed-hex",
                SEED_HEX,
            ]),
            &[
                (ENV_P2P_SERVER_URL, "http://127.0.0.1:9090"),
                (ENV_P2P_SERVER_TOKEN, "env-token"),
            ],
        )
        .unwrap();
        let p2p = config.p2p.expect("p2p config present");
        let server = p2p.server.expect("env server must apply");
        assert_eq!(server.url, "http://127.0.0.1:9090");
        assert_eq!(server.token, "env-token");
    }

    #[test]
    fn env_alone_enables_p2p_without_cli_flags() {
        let config = parse_args_with_env(
            args(&["--data-dir", "/tmp/x"]),
            &[(ENV_P2P_SECRET_KEY_SEED_HEX, SEED_HEX)],
        )
        .unwrap();
        let p2p = config
            .p2p
            .expect("env-only P2P config must enable the lane");
        assert!(matches!(p2p.identity, ReceiverIdentity::Seed(s) if s == [0xab; 32]));
    }

    #[test]
    fn explicit_env_false_reenables_seed_disabled_relay() {
        let config = parse_args_with_env(
            args(&[
                "--data-dir",
                "/tmp/x",
                "--p2p-secret-key-seed-hex",
                SEED_HEX,
            ]),
            &[(ENV_P2P_RELAY_DISABLED, "0")],
        )
        .unwrap();
        let p2p = config.p2p.expect("p2p config present");
        assert!(
            !p2p.relay_disabled,
            "explicit env false must override the seed-implied default"
        );
        assert!(
            p2p.discovery_disabled,
            "unset discovery flag must keep the seed-implied default"
        );
    }

    #[test]
    fn cli_disable_flag_wins_over_env_false() {
        let config = parse_args_with_env(
            args(&[
                "--data-dir",
                "/tmp/x",
                "--p2p-secret-key-seed-hex",
                SEED_HEX,
                "--p2p-relay-disabled",
            ]),
            &[(ENV_P2P_RELAY_DISABLED, "0")],
        )
        .unwrap();
        assert!(
            config.p2p.unwrap().relay_disabled,
            "CLI disable flag must win over an env re-enable"
        );
    }

    #[test]
    fn empty_flag_value_treated_as_absent() {
        // Lookup semantics: empty/whitespace values are absent, so an empty
        // server URL with a real token is a pairing error, not an empty URL.
        let err = parse_args(args(&[
            "--data-dir",
            "/tmp/x",
            "--p2p-server-url",
            "  ",
            "--p2p-server-token",
            "secret-token",
        ]))
        .unwrap_err();
        assert!(err.contains("must be set together"), "got: {err}");
        assert!(
            err.contains("--p2p-server-url"),
            "error must be decorated with flag names, got: {err}"
        );
    }
}
