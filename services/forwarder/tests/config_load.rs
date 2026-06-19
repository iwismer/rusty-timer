use forwarder::config::{load_config_from_str, validate_retention_settings};
use std::io::Write;

fn write_token_file(token: &str) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().expect("create temp file");
    write!(file, "{token}").expect("write token");
    file
}

fn minimal_config(extra: &str) -> (String, tempfile::NamedTempFile) {
    let token_file = write_token_file("my-bearer-token");
    let toml = format!(
        r#"
schema_version = 1

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"

{extra}
"#,
        token_file.path().display()
    );
    (toml, token_file)
}

#[test]
fn valid_minimal_config_loads_ok() {
    let (toml, token_file) = minimal_config("");

    let cfg = load_config_from_str(&toml, token_file.path()).expect("should load");

    assert_eq!(cfg.schema_version, 1);
    assert_eq!(cfg.token, "my-bearer-token");
    assert_eq!(cfg.readers.len(), 1);
    assert_eq!(cfg.readers[0].target, "192.168.2.156:10000");
}

#[test]
fn missing_schema_version_fails() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );

    let result = load_config_from_str(&toml, token_file.path());

    assert!(result.is_err(), "missing schema_version must fail");
}

#[test]
fn wrong_schema_version_fails() {
    let (toml, token_file) = minimal_config("");
    let toml = toml.replace("schema_version = 1", "schema_version = 2");

    let result = load_config_from_str(&toml, token_file.path());

    assert!(result.is_err(), "schema_version != 1 must fail");
}

#[test]
fn missing_auth_token_file_fails() {
    let toml = r#"
schema_version = 1

[[readers]]
target = "192.168.2.156:10000"
"#;

    let result = load_config_from_str(toml, std::path::Path::new("/nonexistent"));

    assert!(result.is_err(), "missing auth.token_file must fail");
}

#[test]
fn missing_readers_section_fails() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[auth]
token_file = "{}"
"#,
        token_file.path().display()
    );

    let result = load_config_from_str(&toml, token_file.path());

    assert!(result.is_err(), "missing readers section must fail");
}

#[test]
fn empty_readers_array_fails() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[auth]
token_file = "{}"

readers = []
"#,
        token_file.path().display()
    );

    let result = load_config_from_str(&toml, token_file.path());

    assert!(result.is_err(), "empty readers array must fail");
}

#[test]
fn display_name_is_loaded_when_present() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1
display_name = "Start Line"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );

    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();

    assert_eq!(cfg.display_name.as_deref(), Some("Start Line"));
}

#[test]
fn journal_defaults_match_p2p_cutover_policy() {
    let (toml, token_file) = minimal_config("");

    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();

    assert_eq!(
        cfg.journal.sqlite_path,
        "/var/lib/rusty-timer/forwarder.sqlite3"
    );
    assert_eq!(cfg.journal.prune_watermark_pct, 80);
    assert_eq!(cfg.journal.min_retention_secs, 7 * 24 * 60 * 60);
    assert_eq!(cfg.journal.max_retention_secs, 30 * 24 * 60 * 60);
    assert_eq!(cfg.journal.emergency_free_disk_bytes, 1_000_000_000);
    assert_eq!(cfg.journal.emergency_max_rows, 1_000_000);
}

#[test]
fn custom_retention_config_loads() {
    let (toml, token_file) = minimal_config(
        r#"
[journal]
min_retention = "2d"
max_retention = "9d"
emergency_free_disk_bytes = 1234
emergency_max_rows = 42
"#,
    );

    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();

    assert_eq!(cfg.journal.min_retention_secs, 2 * 24 * 60 * 60);
    assert_eq!(cfg.journal.max_retention_secs, 9 * 24 * 60 * 60);
    assert_eq!(cfg.journal.emergency_free_disk_bytes, 1234);
    assert_eq!(cfg.journal.emergency_max_rows, 42);
}

#[test]
fn invalid_retention_suffix_fails() {
    let (toml, token_file) = minimal_config(
        r#"
[journal]
min_retention = "7x"
"#,
    );

    let result = load_config_from_str(&toml, token_file.path());

    assert!(result.is_err(), "invalid retention suffix must fail");
}

#[test]
fn max_retention_less_than_min_fails() {
    let (toml, token_file) = minimal_config(
        r#"
[journal]
min_retention = "10d"
max_retention = "2d"
"#,
    );

    let result = load_config_from_str(&toml, token_file.path());

    assert!(result.is_err(), "max < min retention must fail");
}

#[test]
fn validate_retention_settings_rejects_non_positive_emergency_rows() {
    let err = validate_retention_settings(None, None, Some(0)).unwrap_err();

    assert!(err.contains("emergency_max_rows"));
}

#[test]
fn status_http_defaults_to_loopback() {
    let (toml, token_file) = minimal_config("");

    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();

    assert_eq!(cfg.status_http.bind, "127.0.0.1:8080");
}

#[test]
fn control_power_actions_default_to_disabled() {
    let (toml, token_file) = minimal_config("");

    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();

    assert!(!cfg.control.allow_power_actions);
}

#[test]
fn p2p_defaults_to_disabled_local_only_mode() {
    let (toml, token_file) = minimal_config("");

    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();

    assert!(!cfg.p2p.enabled);
    assert_eq!(cfg.p2p.bind_addr_v4, "0.0.0.0:0");
    assert!(cfg.p2p.static_allowed_receivers.is_empty());
}

#[test]
fn p2p_parses_loopback_deterministic_options() {
    let (toml, token_file) = minimal_config(
        r#"
[p2p]
enabled = true
secret_key_seed_hex = "0101010101010101010101010101010101010101010101010101010101010101"
bind_addr_v4 = "127.0.0.1:0"
relay_disabled = true
discovery_disabled = true
max_concurrent_bidi_streams = 64
static_allowed_receivers = ["receiver-node-id"]
allowlist_cache_path = "/tmp/forwarder-p2p-allowlist.cache"
server_url = "http://127.0.0.1:9999"
server_token_file = "/tmp/thin-token"
allowlist_poll_interval_secs = 5
"#,
    );

    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();

    assert!(cfg.p2p.enabled);
    assert_eq!(cfg.p2p.bind_addr_v4, "127.0.0.1:0");
    assert!(cfg.p2p.relay_disabled);
    assert!(cfg.p2p.discovery_disabled);
    assert_eq!(cfg.p2p.max_concurrent_bidi_streams, Some(64));
    assert_eq!(cfg.p2p.static_allowed_receivers, ["receiver-node-id"]);
    assert_eq!(cfg.p2p.allowlist_poll_interval_secs, 5);
}

#[test]
fn p2p_rejects_mutually_exclusive_secret_key_sources() {
    let (toml, token_file) = minimal_config(
        r#"
[p2p]
enabled = true
secret_key_path = "/tmp/key"
secret_key_seed_hex = "0101010101010101010101010101010101010101010101010101010101010101"
static_allowed_receivers = ["receiver-node-id"]
"#,
    );

    let result = load_config_from_str(&toml, token_file.path());

    assert!(
        result.is_err(),
        "mutually exclusive P2P key sources must fail"
    );
}

#[test]
fn disabled_reader_is_retained_for_status_visibility() {
    let (toml, token_file) = minimal_config(
        r#"
[[readers]]
target = "192.168.2.157:10000"
enabled = false
local_fallback_port = 12000
"#,
    );

    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();

    assert_eq!(cfg.readers.len(), 2);
    assert!(!cfg.readers[1].enabled);
    assert_eq!(cfg.readers[1].local_fallback_port, Some(12000));
}
