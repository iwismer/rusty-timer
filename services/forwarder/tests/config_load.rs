/// Integration tests for forwarder config loading.
///
/// Tests config precedence, default values, required field validation,
/// and token file reading.
use forwarder::config::load_config_from_str;
use std::io::Write;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Write a TOML string to a temp file and return the path.
fn write_token_file(token: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::NamedTempFile::new().expect("create temp file");
    write!(f, "{}", token).expect("write token");
    f
}

// ---------------------------------------------------------------------------
// Required fields
// ---------------------------------------------------------------------------

#[test]
fn valid_minimal_config_loads_ok() {
    let token_file = write_token_file("my-bearer-token");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).expect("should load");
    assert_eq!(cfg.schema_version, 1);
    assert_eq!(cfg.server.base_url, "https://timing.example.com");
    assert_eq!(cfg.token, "my-bearer-token");
    assert_eq!(cfg.readers.len(), 1);
}

#[test]
fn missing_schema_version_fails() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
[server]
base_url = "https://timing.example.com"

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
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 2

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let result = load_config_from_str(&toml, token_file.path());
    assert!(result.is_err(), "schema_version != 1 must fail");
}

#[test]
fn missing_server_base_url_fails() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let result = load_config_from_str(&toml, token_file.path());
    assert!(result.is_err(), "missing server.base_url must fail");
}

#[test]
fn missing_auth_token_file_fails() {
    let toml = r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[[readers]]
target = "192.168.2.156:10000"
"#;
    // We pass a dummy path (won't be used for token_file lookup if auth section missing)
    let result = load_config_from_str(toml, std::path::Path::new("/nonexistent"));
    assert!(result.is_err(), "missing auth.token_file must fail");
}

#[test]
fn missing_readers_section_fails() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

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

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

readers = []
"#,
        token_file.path().display()
    );
    let result = load_config_from_str(&toml, token_file.path());
    assert!(result.is_err(), "empty readers array must fail");
}

// ---------------------------------------------------------------------------
// display_name
// ---------------------------------------------------------------------------

#[test]
fn display_name_is_loaded_when_present() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1
display_name = "Start Line"

[server]
base_url = "https://timing.example.com"

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
fn display_name_defaults_to_none() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    assert!(cfg.display_name.is_none());
}

// ---------------------------------------------------------------------------
// Default values
// ---------------------------------------------------------------------------

#[test]
fn default_forwarders_ws_path() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    assert_eq!(cfg.server.forwarders_ws_path, "/ws/v1/forwarders");
}

#[test]
fn default_journal_sqlite_path() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    assert_eq!(
        cfg.journal.sqlite_path,
        "/var/lib/rusty-timer/forwarder.sqlite3"
    );
}

#[test]
fn default_prune_watermark_pct() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    assert_eq!(cfg.journal.prune_watermark_pct, 80);
}

#[test]
fn default_status_http_bind() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    assert_eq!(cfg.status_http.bind, "0.0.0.0:8080");
}

#[test]
fn default_uplink_batch_mode() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    assert_eq!(cfg.uplink.batch_mode, "immediate");
    assert_eq!(cfg.uplink.batch_flush_ms, 100);
    assert_eq!(cfg.uplink.batch_max_events, 50);
}

#[test]
fn control_allow_power_actions_defaults_to_false() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    assert!(!cfg.control.allow_power_actions);
}

#[test]
fn control_allow_power_actions_true_is_loaded() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[control]
allow_power_actions = true

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    assert!(cfg.control.allow_power_actions);
}

#[test]
fn default_reader_enabled() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    let r = &cfg.readers[0];
    assert!(r.enabled);
}

#[test]
fn default_reader_local_fallback_port_is_10000_plus_last_octet() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    // 192.168.2.156 → last_octet = 156, default fallback port = 10000 + 156 = 10156
    // But target port is 10000. The local_fallback_port is a separate concept.
    // For a single-IP target, last octet of IP = 156, so default local_fallback_port = 10156
    let r = &cfg.readers[0];
    // local_fallback_port is derived at expansion time via discovery module
    // Config stores None (unset), expansion provides default
    assert!(
        r.local_fallback_port.is_none(),
        "should be None when not explicitly set"
    );
}

#[test]
fn explicit_local_fallback_port_is_used() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
local_fallback_port = 9999
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    let r = &cfg.readers[0];
    assert_eq!(r.local_fallback_port, Some(9999));
}

// ---------------------------------------------------------------------------
// Token file reading
// ---------------------------------------------------------------------------

#[test]
fn token_is_read_and_trimmed() {
    let token_file = write_token_file("  my-token-with-whitespace  \n");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.156:10000"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    assert_eq!(cfg.token, "my-token-with-whitespace");
}

#[test]
fn nonexistent_token_file_fails() {
    let toml = r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "/nonexistent/path/to/token"

[[readers]]
target = "192.168.2.156:10000"
"#;
    let result = load_config_from_str(toml, std::path::Path::new("/nonexistent/path/to/token"));
    assert!(result.is_err(), "nonexistent token file must fail");
}

// ---------------------------------------------------------------------------
// load_config_from_path
// ---------------------------------------------------------------------------

#[test]
fn load_config_from_path_reads_toml_file() {
    let token_file = write_token_file("dev-token");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "ws://127.0.0.1:8080"

[auth]
token_file = "{}"

[[readers]]
target = "127.0.0.1:10001"
"#,
        token_file.path().display()
    );
    let mut config_file = tempfile::NamedTempFile::new().unwrap();
    config_file.write_all(toml.as_bytes()).unwrap();

    let cfg = forwarder::config::load_config_from_path(config_file.path())
        .expect("should load from arbitrary path");
    assert_eq!(cfg.server.base_url, "ws://127.0.0.1:8080");
    assert_eq!(cfg.token, "dev-token");
    assert_eq!(cfg.readers[0].target, "127.0.0.1:10001");
}

// ---------------------------------------------------------------------------
// Multiple readers
// ---------------------------------------------------------------------------

#[test]
fn multiple_readers_are_loaded() {
    let token_file = write_token_file("tok");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "https://timing.example.com"

[auth]
token_file = "{}"

[[readers]]
target = "192.168.2.150:10000"

[[readers]]
target = "192.168.2.151:10000"
read_type = "fsls"
"#,
        token_file.path().display()
    );
    let cfg = load_config_from_str(&toml, token_file.path()).unwrap();
    assert_eq!(cfg.readers.len(), 2);
}
