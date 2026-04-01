# PiSugar 3 Plus UPS Support — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add optional PiSugar 3 Plus UPS monitoring to the forwarder, with battery status flowing through the server to the receiver and dashboard UIs.

**Architecture:** The forwarder polls the pisugar-server daemon via TCP text protocol, sends `ForwarderUpsStatus` WsMessages upstream. The server caches UPS state, fans out via `DashboardEvent` SSE (dashboard) and sentinel `ReadEvent`s (receivers). UIs show battery indicators with color coding and critical battery warnings.

**Tech Stack:** Rust (tokio, serde), SvelteKit (Svelte 5 runes), PostgreSQL migrations, bash scripting, Python (cloud-init generator)

**Spec:** `docs/specs/2026-04-01-pisugar3-ups-support.md`

---

## File Map

### New files
- `crates/rt-protocol/src/ups.rs` — `UpsStatus` struct + `same_readings()` + serde tests
- `services/forwarder/src/pisugar_client.rs` — async TCP client for pisugar-server daemon
- `services/forwarder/src/ups_task.rs` — background polling task + change detection + upstream send
- `services/server/migrations/0011_forwarder_ups_events.sql` — power transition logging table
- `apps/shared-ui/src/components/BatteryIndicator.svelte` — reusable battery icon component
- `apps/shared-ui/src/components/LowBatteryBanner.svelte` — critical battery warning banner

### Modified files
- `crates/rt-protocol/src/lib.rs` — add `ForwarderUpsStatus` WsMessage variant, sentinel constant, re-export ups module
- `services/forwarder/src/config.rs` — add `RawUpsConfig`, `UpsConfig`, validation
- `services/forwarder/src/ui_events.rs` — add `UpsStatusChanged` variant
- `services/forwarder/src/status_http.rs` — add `ups_status` to `SubsystemStatus`, add `"ups"` config section handler
- `services/forwarder/src/main.rs` — spawn UPS background task, create channel
- `services/forwarder/src/uplink_task.rs` — accept UPS channel, send UPS burst on reconnect, drain in main loop
- `services/server/src/state.rs` — add `ForwarderUpsCache` + `CachedUpsState` to `AppState`
- `services/server/src/dashboard_events.rs` — add `ForwarderUpsUpdated` variant
- `services/server/src/http/sse.rs` — add `"forwarder_ups_updated"` SSE event type
- `services/server/src/ws_forwarder.rs` — handle `ForwarderUpsStatus`, cache, fanout, disconnect cleanup
- `services/server/src/ws_receiver.rs` — forward `__forwarder_ups_status` sentinel, send initial UPS snapshot
- `services/receiver/src/session.rs` — handle `ForwarderUpsStatus` WsMessage, emit to UI
- `services/receiver/src/ui_events.rs` — add `ForwarderUpsUpdated` variant
- `apps/receiver-ui/src/lib/api.ts` — add `UpsStatus` + `ForwarderUpsState` types
- `apps/receiver-ui/src/lib/sse.ts` — add `onForwarderUpsUpdated` callback
- `apps/receiver-ui/src/lib/store.svelte.ts` — add `upsState` map, wire SSE callback
- `apps/receiver-ui/src/lib/components/ForwardersTab.svelte` — battery column + detail card
- `apps/receiver-ui/src/lib/components/StreamsTab.svelte` — battery icon in stream rows
- `apps/server-ui/src/lib/sse.ts` — add `forwarder_ups_updated` SSE listener
- `apps/server-ui/src/lib/stores.ts` — add `upsStateStore`
- `apps/server-ui/src/routes/+page.svelte` — battery column in forwarder groups
- `apps/forwarder-ui/src/routes/+page.svelte` — battery indicator in status area
- `apps/forwarder-ui/src/lib/sse.ts` — add `ups_status_changed` listener
- `apps/shared-ui/src/lib/forwarder-config-layout.ts` — add UPS config section
- `apps/server-ui/src/routes/sbc-setup/+page.svelte` — add UPS toggle
- `apps/server-ui/src/lib/sbc-setup/generate.ts` — add `ups_enabled` to cloud-init generation
- `deploy/sbc/rt-setup.sh` — add UPS setup (pisugar-server install, I2C, systemd)
- `scripts/sbc_cloud_init.py` — add `ups_enabled` field

---

## Task 1: UpsStatus shared type (rt-protocol)

**Files:**
- Create: `crates/rt-protocol/src/ups.rs`
- Modify: `crates/rt-protocol/src/lib.rs`

- [ ] **Step 1: Write tests for UpsStatus and same_readings()**

Create `crates/rt-protocol/src/ups.rs`:

```rust
use serde::{Deserialize, Serialize};

/// Raw UPS telemetry. Integer types avoid float comparison issues.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpsStatus {
    pub battery_percent: u8,
    pub battery_voltage_mv: u16,
    pub charging: bool,
    pub power_plugged: bool,
    pub temperature_cdeg: i16,
    pub sampled_at: i64,
}

impl UpsStatus {
    /// Compare readings only, ignoring `sampled_at` which changes every poll.
    pub fn same_readings(&self, other: &Self) -> bool {
        self.battery_percent == other.battery_percent
            && self.battery_voltage_mv == other.battery_voltage_mv
            && self.charging == other.charging
            && self.power_plugged == other.power_plugged
            && self.temperature_cdeg == other.temperature_cdeg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_readings_ignores_sampled_at() {
        let a = UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1000,
        };
        let b = UpsStatus { sampled_at: 2000, ..a.clone() };
        assert!(a.same_readings(&b));
    }

    #[test]
    fn same_readings_detects_percent_change() {
        let a = UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1000,
        };
        let b = UpsStatus { battery_percent: 72, ..a.clone() };
        assert!(!a.same_readings(&b));
    }

    #[test]
    fn same_readings_detects_voltage_change() {
        let a = UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1000,
        };
        let b = UpsStatus { battery_voltage_mv: 3860, ..a.clone() };
        assert!(!a.same_readings(&b));
    }

    #[test]
    fn same_readings_detects_charging_change() {
        let a = UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1000,
        };
        let b = UpsStatus { charging: false, ..a.clone() };
        assert!(!a.same_readings(&b));
    }

    #[test]
    fn same_readings_detects_power_plugged_change() {
        let a = UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1000,
        };
        let b = UpsStatus { power_plugged: false, ..a.clone() };
        assert!(!a.same_readings(&b));
    }

    #[test]
    fn same_readings_detects_temperature_change() {
        let a = UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1000,
        };
        let b = UpsStatus { temperature_cdeg: 4300, ..a.clone() };
        assert!(!a.same_readings(&b));
    }

    #[test]
    fn ups_status_round_trip_serde() {
        let status = UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1711929600000,
        };
        let json = serde_json::to_string(&status).unwrap();
        let parsed: UpsStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, parsed);
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p rt-protocol --lib ups`
Expected: PASS (7 tests)

- [ ] **Step 3: Add ForwarderUpsStatus WsMessage variant and sentinel constant**

In `crates/rt-protocol/src/lib.rs`:

Add at the top with other modules:
```rust
pub mod ups;
pub use ups::UpsStatus;
```

Add the sentinel constant after the existing ones (~line 137):
```rust
/// Sentinel `read_type` for forwarder UPS status control messages.
/// When a `ReadEvent` carries this type, its `raw_frame` contains
/// JSON with `forwarder_id`, `available`, and optional `UpsStatus`.
pub const FORWARDER_UPS_STATUS_READ_TYPE: &str = "__forwarder_ups_status";
```

Add the payload struct (near other forwarder-related structs):
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwarderUpsStatus {
    pub forwarder_id: String,
    pub available: bool,
    pub status: Option<UpsStatus>,
}
```

Add the WsMessage variant (in the `WsMessage` enum, after `ReceiverReaderDownloadProgress`):
```rust
ForwarderUpsStatus(ForwarderUpsStatus),
```

- [ ] **Step 4: Add serde round-trip test for ForwarderUpsStatus WsMessage**

Add to the existing test module in `lib.rs` (or a new test block):
```rust
#[test]
fn forwarder_ups_status_round_trip() {
    let msg = WsMessage::ForwarderUpsStatus(ForwarderUpsStatus {
        forwarder_id: "fwd-1".to_owned(),
        available: true,
        status: Some(UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1711929600000,
        }),
    });
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"kind\":\"forwarder_ups_status\""));
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}

#[test]
fn forwarder_ups_status_unavailable_round_trip() {
    let msg = WsMessage::ForwarderUpsStatus(ForwarderUpsStatus {
        forwarder_id: "fwd-1".to_owned(),
        available: false,
        status: None,
    });
    let json = serde_json::to_string(&msg).unwrap();
    let parsed: WsMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, parsed);
}
```

- [ ] **Step 5: Run all rt-protocol tests**

Run: `cargo test -p rt-protocol`
Expected: PASS (all existing + 9 new)

- [ ] **Step 6: Commit**

```bash
git add crates/rt-protocol/src/ups.rs crates/rt-protocol/src/lib.rs
git commit -m "feat(rt-protocol): add UpsStatus type, ForwarderUpsStatus WsMessage variant, and sentinel constant"
```

---

## Task 2: Forwarder config — UPS section

**Files:**
- Modify: `services/forwarder/src/config.rs`

- [ ] **Step 1: Add RawUpsConfig and UpsConfig structs**

In `config.rs`, add after `RawReaderConfig` (~line 144):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawUpsConfig {
    pub enabled: Option<bool>,
    pub daemon_addr: Option<String>,
    pub poll_interval_secs: Option<u64>,
    pub upstream_heartbeat_secs: Option<u64>,
}
```

Add after `ReaderConfig` (~line 80):

```rust
#[derive(Debug, Clone)]
pub struct UpsConfig {
    pub enabled: bool,
    pub daemon_addr: String,
    pub poll_interval_secs: u64,
    pub upstream_heartbeat_secs: u64,
}
```

Add `ups: Option<RawUpsConfig>` to `RawConfig` (after `update` field, ~line 97):

```rust
pub ups: Option<RawUpsConfig>,
```

Add `pub ups: UpsConfig` to `ForwarderConfig` (after `update`, ~line 36):

```rust
pub ups: UpsConfig,
```

- [ ] **Step 2: Add validation logic in load_config_from_str**

In `load_config_from_str`, add the UPS section handling after the `update` section (before `readers`):

```rust
let ups = match raw.ups {
    Some(u) => {
        let enabled = u.enabled.unwrap_or(false);
        let daemon_addr = u.daemon_addr.unwrap_or_else(|| "127.0.0.1:8423".to_owned());
        let poll_interval_secs = u.poll_interval_secs.unwrap_or(5);
        let upstream_heartbeat_secs = u.upstream_heartbeat_secs.unwrap_or(60);
        if poll_interval_secs < 1 || poll_interval_secs > 60 {
            return Err(ConfigError::InvalidValue(
                "ups.poll_interval_secs must be between 1 and 60".to_owned(),
            ));
        }
        if upstream_heartbeat_secs < 10 || upstream_heartbeat_secs > 300 {
            return Err(ConfigError::InvalidValue(
                "ups.upstream_heartbeat_secs must be between 10 and 300".to_owned(),
            ));
        }
        // Validate daemon_addr is a valid host:port
        if daemon_addr.parse::<std::net::SocketAddr>().is_err() {
            // Try as host:port (might be a hostname)
            let parts: Vec<&str> = daemon_addr.rsplitn(2, ':').collect();
            if parts.len() != 2 || parts[0].parse::<u16>().is_err() {
                return Err(ConfigError::InvalidValue(format!(
                    "ups.daemon_addr must be a valid host:port, got '{}'",
                    daemon_addr
                )));
            }
        }
        UpsConfig {
            enabled,
            daemon_addr,
            poll_interval_secs,
            upstream_heartbeat_secs,
        }
    }
    None => UpsConfig {
        enabled: false,
        daemon_addr: "127.0.0.1:8423".to_owned(),
        poll_interval_secs: 5,
        upstream_heartbeat_secs: 60,
    },
};
```

And include `ups` in the `ForwarderConfig` construction.

- [ ] **Step 3: Write config tests**

Add tests to the existing test module:

```rust
#[test]
fn ups_section_absent_defaults_to_disabled() {
    let toml = minimal_toml(); // existing helper
    let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
    assert!(!cfg.ups.enabled);
    assert_eq!(cfg.ups.daemon_addr, "127.0.0.1:8423");
    assert_eq!(cfg.ups.poll_interval_secs, 5);
    assert_eq!(cfg.ups.upstream_heartbeat_secs, 60);
}

#[test]
fn ups_section_enabled_with_defaults() {
    let toml = format!("{}\n[ups]\nenabled = true\n", minimal_toml());
    let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
    assert!(cfg.ups.enabled);
    assert_eq!(cfg.ups.daemon_addr, "127.0.0.1:8423");
}

#[test]
fn ups_section_custom_addr() {
    let toml = format!(
        "{}\n[ups]\nenabled = true\ndaemon_addr = \"192.168.1.10:8423\"\n",
        minimal_toml()
    );
    let cfg = load_config_from_str(&toml, Path::new("/tmp/test.toml")).unwrap();
    assert_eq!(cfg.ups.daemon_addr, "192.168.1.10:8423");
}

#[test]
fn ups_poll_interval_out_of_range_rejected() {
    let toml = format!("{}\n[ups]\npoll_interval_secs = 0\n", minimal_toml());
    assert!(load_config_from_str(&toml, Path::new("/tmp/test.toml")).is_err());

    let toml = format!("{}\n[ups]\npoll_interval_secs = 61\n", minimal_toml());
    assert!(load_config_from_str(&toml, Path::new("/tmp/test.toml")).is_err());
}

#[test]
fn ups_heartbeat_out_of_range_rejected() {
    let toml = format!("{}\n[ups]\nupstream_heartbeat_secs = 9\n", minimal_toml());
    assert!(load_config_from_str(&toml, Path::new("/tmp/test.toml")).is_err());

    let toml = format!("{}\n[ups]\nupstream_heartbeat_secs = 301\n", minimal_toml());
    assert!(load_config_from_str(&toml, Path::new("/tmp/test.toml")).is_err());
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p forwarder --lib config`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add services/forwarder/src/config.rs
git commit -m "feat(forwarder): add optional [ups] config section with validation"
```

---

## Task 3: Forwarder pisugar TCP client

**Files:**
- Create: `services/forwarder/src/pisugar_client.rs`

- [ ] **Step 1: Write the pisugar client with mock TCP tests**

Create `services/forwarder/src/pisugar_client.rs`:

```rust
//! Async TCP client for the pisugar-server daemon text protocol.

use rt_protocol::UpsStatus;
use std::io;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// Errors from pisugar daemon communication.
#[derive(Debug, thiserror::Error)]
pub enum PisugarError {
    #[error("connection failed: {0}")]
    Connect(io::Error),
    #[error("send failed: {0}")]
    Send(io::Error),
    #[error("read failed: {0}")]
    Read(io::Error),
    #[error("unexpected response for '{command}': {response}")]
    UnexpectedResponse { command: String, response: String },
    #[error("parse error for '{command}': {detail}")]
    Parse { command: String, detail: String },
}

/// Poll the pisugar-server daemon at the given address for current UPS status.
///
/// Opens a fresh TCP connection per poll. The daemon protocol is stateless
/// (no session), so this is simpler and more robust than maintaining a
/// persistent connection with reconnect logic.
pub async fn poll_status(addr: &str) -> Result<UpsStatus, PisugarError> {
    let stream = tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(addr))
        .await
        .map_err(|_| PisugarError::Connect(io::Error::new(io::ErrorKind::TimedOut, "connect timeout")))?
        .map_err(PisugarError::Connect)?;

    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let battery_percent = send_and_parse_u8(&mut writer, &mut reader, "get battery", "battery").await?;
    let battery_voltage_mv = send_and_parse_voltage_mv(&mut writer, &mut reader).await?;
    let charging = send_and_parse_bool(&mut writer, &mut reader, "get battery_charging", "battery_charging").await?;
    let power_plugged = send_and_parse_bool(&mut writer, &mut reader, "get battery_power_plugged", "battery_power_plugged").await?;
    let temperature_cdeg = send_and_parse_temperature_cdeg(&mut writer, &mut reader).await?;

    let sampled_at = chrono::Utc::now().timestamp_millis();

    Ok(UpsStatus {
        battery_percent,
        battery_voltage_mv,
        charging,
        power_plugged,
        temperature_cdeg,
        sampled_at,
    })
}

async fn send_command(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    command: &str,
) -> Result<String, PisugarError> {
    writer
        .write_all(format!("{}\n", command).as_bytes())
        .await
        .map_err(PisugarError::Send)?;
    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
        .await
        .map_err(|_| PisugarError::Read(io::Error::new(io::ErrorKind::TimedOut, "read timeout")))?
        .map_err(PisugarError::Read)?;
    Ok(line.trim().to_owned())
}

fn parse_value<'a>(response: &'a str, prefix: &str, command: &str) -> Result<&'a str, PisugarError> {
    let expected_prefix = format!("{}: ", prefix);
    response.strip_prefix(&expected_prefix).ok_or_else(|| PisugarError::UnexpectedResponse {
        command: command.to_owned(),
        response: response.to_owned(),
    })
}

async fn send_and_parse_u8(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    command: &str,
    prefix: &str,
) -> Result<u8, PisugarError> {
    let response = send_command(writer, reader, command).await?;
    let value_str = parse_value(&response, prefix, command)?;
    // The daemon may return a float like "73.5" for battery percent
    let float_val: f64 = value_str.parse().map_err(|_| PisugarError::Parse {
        command: command.to_owned(),
        detail: format!("cannot parse '{}' as number", value_str),
    })?;
    Ok(float_val.round() as u8)
}

async fn send_and_parse_bool(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
    command: &str,
    prefix: &str,
) -> Result<bool, PisugarError> {
    let response = send_command(writer, reader, command).await?;
    let value_str = parse_value(&response, prefix, command)?;
    match value_str {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(PisugarError::Parse {
            command: command.to_owned(),
            detail: format!("expected 'true' or 'false', got '{}'", value_str),
        }),
    }
}

async fn send_and_parse_voltage_mv(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> Result<u16, PisugarError> {
    let response = send_command(writer, reader, "get battery_v").await?;
    let value_str = parse_value(&response, "battery_v", "get battery_v")?;
    let volts: f64 = value_str.parse().map_err(|_| PisugarError::Parse {
        command: "get battery_v".to_owned(),
        detail: format!("cannot parse '{}' as float", value_str),
    })?;
    Ok((volts * 1000.0).round() as u16)
}

async fn send_and_parse_temperature_cdeg(
    writer: &mut tokio::net::tcp::OwnedWriteHalf,
    reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>,
) -> Result<i16, PisugarError> {
    let response = send_command(writer, reader, "get temperature").await?;
    let value_str = parse_value(&response, "temperature", "get temperature")?;
    let celsius: f64 = value_str.parse().map_err(|_| PisugarError::Parse {
        command: "get temperature".to_owned(),
        detail: format!("cannot parse '{}' as float", value_str),
    })?;
    Ok((celsius * 100.0).round() as i16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;

    /// Start a mock pisugar-server that responds to the 5 known commands.
    async fn start_mock_daemon(
        battery: &str,
        battery_v: &str,
        charging: &str,
        power_plugged: &str,
        temperature: &str,
    ) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let responses: Vec<(String, String)> = vec![
            ("get battery".to_owned(), format!("battery: {}", battery)),
            ("get battery_v".to_owned(), format!("battery_v: {}", battery_v)),
            ("get battery_charging".to_owned(), format!("battery_charging: {}", charging)),
            ("get battery_power_plugged".to_owned(), format!("battery_power_plugged: {}", power_plugged)),
            ("get temperature".to_owned(), format!("temperature: {}", temperature)),
        ];

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);

            for (expected_cmd, response) in &responses {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                assert_eq!(line.trim(), *expected_cmd);
                writer.write_all(format!("{}\n", response).as_bytes()).await.unwrap();
            }
        });

        addr
    }

    #[tokio::test]
    async fn poll_status_parses_all_five_fields() {
        let addr = start_mock_daemon("73", "3.87", "true", "true", "42.5").await;
        let status = poll_status(&addr).await.unwrap();
        assert_eq!(status.battery_percent, 73);
        assert_eq!(status.battery_voltage_mv, 3870);
        assert!(status.charging);
        assert!(status.power_plugged);
        assert_eq!(status.temperature_cdeg, 4250);
        assert!(status.sampled_at > 0);
    }

    #[tokio::test]
    async fn poll_status_rounds_fractional_battery_percent() {
        let addr = start_mock_daemon("73.5", "4.12", "false", "false", "31.2").await;
        let status = poll_status(&addr).await.unwrap();
        assert_eq!(status.battery_percent, 74); // 73.5 rounds to 74
        assert_eq!(status.battery_voltage_mv, 4120);
        assert!(!status.charging);
        assert!(!status.power_plugged);
        assert_eq!(status.temperature_cdeg, 3120);
    }

    #[tokio::test]
    async fn poll_status_connection_refused() {
        // Connect to a port nothing is listening on
        let result = poll_status("127.0.0.1:1").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PisugarError::Connect(_)));
    }

    #[tokio::test]
    async fn voltage_conversion_precision() {
        // 3.87V -> 3870mV
        let addr = start_mock_daemon("50", "3.87", "false", "true", "0.0").await;
        let status = poll_status(&addr).await.unwrap();
        assert_eq!(status.battery_voltage_mv, 3870);
    }

    #[tokio::test]
    async fn temperature_conversion_negative() {
        let addr = start_mock_daemon("50", "3.80", "false", "true", "-5.5").await;
        let status = poll_status(&addr).await.unwrap();
        assert_eq!(status.temperature_cdeg, -550);
    }
}
```

- [ ] **Step 2: Register the module**

Add `pub mod pisugar_client;` to `services/forwarder/src/main.rs` (or `lib.rs` if one exists).

- [ ] **Step 3: Run tests**

Run: `cargo test -p forwarder --lib pisugar_client`
Expected: PASS (5 tests)

- [ ] **Step 4: Commit**

```bash
git add services/forwarder/src/pisugar_client.rs services/forwarder/src/main.rs
git commit -m "feat(forwarder): add pisugar TCP client with mock daemon tests"
```

---

## Task 4: Forwarder UPS background task

**Files:**
- Create: `services/forwarder/src/ups_task.rs`
- Modify: `services/forwarder/src/ui_events.rs`
- Modify: `services/forwarder/src/status_http.rs`

- [ ] **Step 1: Add UpsStatusChanged variant to ForwarderUiEvent**

In `services/forwarder/src/ui_events.rs`, add to the `ForwarderUiEvent` enum:

```rust
UpsStatusChanged {
    available: bool,
    status: Option<rt_protocol::UpsStatus>,
},
```

Add a test:

```rust
#[test]
fn ups_status_changed_serializes_with_type_tag() {
    let event = ForwarderUiEvent::UpsStatusChanged {
        available: true,
        status: Some(rt_protocol::UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1711929600000,
        }),
    };
    let json: serde_json::Value = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "ups_status_changed");
    assert_eq!(json["available"], true);
    assert_eq!(json["status"]["battery_percent"], 73);
}
```

- [ ] **Step 2: Add ups_status to SubsystemStatus**

In `services/forwarder/src/status_http.rs`, add to `SubsystemStatus`:

```rust
ups_status: Option<UpsStatusState>,
```

Add the state wrapper:
```rust
#[derive(Debug, Clone, Serialize)]
pub struct UpsStatusState {
    pub available: bool,
    pub status: Option<rt_protocol::UpsStatus>,
}
```

Add to `SubsystemStatus::ready()` and `not_ready()`:
```rust
ups_status: None,
```

Add getter/setter methods:
```rust
pub fn set_ups_status(&mut self, state: UpsStatusState) {
    self.ups_status = Some(state);
}

pub fn ups_status(&self) -> Option<&UpsStatusState> {
    self.ups_status.as_ref()
}
```

Update the JSON serialization (in `get_status_handler` or wherever `SubsystemStatus` is serialized) to include `ups_status`.

- [ ] **Step 3: Write the UPS background task**

Create `services/forwarder/src/ups_task.rs`:

```rust
//! Background task that polls the pisugar-server daemon and sends UPS status upstream.

use crate::config::UpsConfig;
use crate::pisugar_client;
use crate::status_http::{StatusServer, UpsStatusState};
use crate::ui_events::ForwarderUiEvent;
use rt_protocol::{ForwarderUpsStatus, UpsStatus};
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tracing::{info, warn};

pub struct UpsTaskHandle {
    pub ups_status_rx: mpsc::UnboundedReceiver<ForwarderUpsStatus>,
}

pub fn spawn_ups_task(
    config: UpsConfig,
    forwarder_id: String,
    status: StatusServer,
    mut shutdown_rx: watch::Receiver<bool>,
) -> UpsTaskHandle {
    let (ups_tx, ups_rx) = mpsc::unbounded_channel::<ForwarderUpsStatus>();

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(config.poll_interval_secs));
        let heartbeat_interval = Duration::from_secs(config.upstream_heartbeat_secs);
        let mut last_heartbeat = tokio::time::Instant::now();
        let mut prev_status: Option<UpsStatus> = None;
        let mut daemon_was_available = true; // assume available initially to detect first failure
        let mut warned_power_unplugged = false;
        let mut warned_low_battery = false;
        let ui_tx = status.ui_sender();

        info!(
            daemon_addr = %config.daemon_addr,
            poll_secs = config.poll_interval_secs,
            heartbeat_secs = config.upstream_heartbeat_secs,
            "UPS monitoring task started"
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("UPS task stopping (shutdown)");
                        return;
                    }
                }
            }

            match pisugar_client::poll_status(&config.daemon_addr).await {
                Ok(current) => {
                    // Transition: unavailable -> available
                    if !daemon_was_available {
                        info!("pisugar daemon reconnected");
                        daemon_was_available = true;
                    }

                    // Warn on power_plugged transition to false (once)
                    if !current.power_plugged && !warned_power_unplugged {
                        warn!(battery_percent = current.battery_percent, "forwarder running on battery power");
                        warned_power_unplugged = true;
                    } else if current.power_plugged && warned_power_unplugged {
                        info!(battery_percent = current.battery_percent, "forwarder power restored");
                        warned_power_unplugged = false;
                    }

                    // Warn on battery crossing below 20% (once)
                    if current.battery_percent < 20 && !current.power_plugged && !warned_low_battery {
                        warn!(battery_percent = current.battery_percent, "low battery warning");
                        warned_low_battery = true;
                    } else if (current.battery_percent >= 20 || current.power_plugged) && warned_low_battery {
                        warned_low_battery = false;
                    }

                    let readings_changed = prev_status
                        .as_ref()
                        .map(|prev| !prev.same_readings(&current))
                        .unwrap_or(true);

                    let heartbeat_due = last_heartbeat.elapsed() >= heartbeat_interval;

                    // Update local status
                    status.set_ups_status(UpsStatusState {
                        available: true,
                        status: Some(current.clone()),
                    }).await;

                    if readings_changed || heartbeat_due {
                        // Send SSE event to local UI
                        let _ = ui_tx.send(ForwarderUiEvent::UpsStatusChanged {
                            available: true,
                            status: Some(current.clone()),
                        });

                        // Send upstream
                        let _ = ups_tx.send(ForwarderUpsStatus {
                            forwarder_id: forwarder_id.clone(),
                            available: true,
                            status: Some(current.clone()),
                        });

                        last_heartbeat = tokio::time::Instant::now();
                    }

                    prev_status = Some(current);
                }
                Err(e) => {
                    // Transition: available -> unavailable (send once)
                    if daemon_was_available {
                        warn!(error = %e, "pisugar daemon unreachable");
                        daemon_was_available = false;

                        let last_known = prev_status.clone();

                        status.set_ups_status(UpsStatusState {
                            available: false,
                            status: last_known.clone(),
                        }).await;

                        let _ = ui_tx.send(ForwarderUiEvent::UpsStatusChanged {
                            available: false,
                            status: last_known.clone(),
                        });

                        let _ = ups_tx.send(ForwarderUpsStatus {
                            forwarder_id: forwarder_id.clone(),
                            available: false,
                            status: last_known,
                        });
                    }
                }
            }
        }
    });

    UpsTaskHandle { ups_status_rx: ups_rx }
}
```

- [ ] **Step 4: Register the module**

Add `pub mod ups_task;` to `services/forwarder/src/main.rs`.

- [ ] **Step 5: Run compilation check**

Run: `cargo check -p forwarder`
Expected: No errors (the `set_ups_status` method needs to be added to `StatusServer` — see step 2)

- [ ] **Step 6: Add set_ups_status to StatusServer**

In `status_http.rs`, add to `StatusServer`:

```rust
pub async fn set_ups_status(&self, state: UpsStatusState) {
    self.subsystem.lock().await.set_ups_status(state);
}
```

- [ ] **Step 7: Run tests**

Run: `cargo test -p forwarder`
Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add services/forwarder/src/ups_task.rs services/forwarder/src/ui_events.rs services/forwarder/src/status_http.rs services/forwarder/src/main.rs
git commit -m "feat(forwarder): add UPS background task with change detection and transition warnings"
```

---

## Task 5: Forwarder main.rs + uplink integration

**Files:**
- Modify: `services/forwarder/src/main.rs`
- Modify: `services/forwarder/src/uplink_task.rs`

- [ ] **Step 1: Spawn UPS task in main.rs**

After the update checker spawn block (~line 392), add:

```rust
// Spawn UPS monitoring task (if enabled)
let ups_handle = if cfg.ups.enabled {
    let ss = status_server.clone();
    let rx = shutdown_rx.clone();
    let fwd_id = forwarder_id.clone();
    Some(crate::ups_task::spawn_ups_task(
        cfg.ups.clone(),
        fwd_id,
        ss,
        rx,
    ))
} else {
    None
};
```

Pass `ups_handle` to the uplink task. Modify the `run_uplink` spawn to pass an additional parameter:

```rust
let ups_status_rx = ups_handle.map(|h| h.ups_status_rx);
```

Add `ups_status_rx` to the `run_uplink` call.

- [ ] **Step 2: Update run_uplink to accept UPS channel**

In `services/forwarder/src/uplink_task.rs`, add a new parameter to `run_uplink`:

```rust
mut ups_status_rx: Option<tokio::sync::mpsc::UnboundedReceiver<rt_protocol::ForwarderUpsStatus>>,
```

After the reader status burst on reconnect, add a UPS status burst:

```rust
// Send initial UPS status burst if available
if let Some(ref mut ups_rx) = ups_status_rx {
    // Drain to get latest
    let mut latest: Option<rt_protocol::ForwarderUpsStatus> = None;
    while let Ok(msg) = ups_rx.try_recv() {
        latest = Some(msg);
    }
    if let Some(msg) = latest {
        let ws_msg = WsMessage::ForwarderUpsStatus(msg);
        if session.send_message(&ws_msg).await.is_err() {
            warn!("failed to send UPS status during initial burst; reconnecting");
            status.set_uplink_connected(false).await;
            continue;
        }
    }
}
```

In the main event select loop, add a branch for UPS messages (alongside reader_status_rx):

```rust
ups_msg = async {
    match ups_status_rx.as_mut() {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
} => {
    if let Some(msg) = ups_msg {
        let ws_msg = WsMessage::ForwarderUpsStatus(msg);
        if session.send_message(&ws_msg).await.is_err() {
            warn!("failed to send UPS status; reconnecting");
            break;
        }
    }
}
```

- [ ] **Step 3: Run compilation check**

Run: `cargo check -p forwarder`
Expected: No errors

- [ ] **Step 4: Commit**

```bash
git add services/forwarder/src/main.rs services/forwarder/src/uplink_task.rs
git commit -m "feat(forwarder): wire UPS task into main + uplink with reconnect burst"
```

---

## Task 6: Forwarder config editing API — UPS section

**Files:**
- Modify: `services/forwarder/src/status_http.rs`

- [ ] **Step 1: Add "ups" match arm to apply_section_update**

In `apply_section_update`, add after the `"readers"` arm (or wherever convenient):

```rust
"ups" => {
    let enabled = optional_bool_field(payload, "enabled")?;
    let daemon_addr = optional_string_field(payload, "daemon_addr")?;
    let poll_interval_secs = optional_u64_field(payload, "poll_interval_secs")?;
    let upstream_heartbeat_secs = optional_u64_field(payload, "upstream_heartbeat_secs")?;

    if let Some(interval) = poll_interval_secs {
        if !(1..=60).contains(&interval) {
            return Err(bad_request_error("poll_interval_secs must be between 1 and 60"));
        }
    }
    if let Some(heartbeat) = upstream_heartbeat_secs {
        if !(10..=300).contains(&heartbeat) {
            return Err(bad_request_error("upstream_heartbeat_secs must be between 10 and 300"));
        }
    }
    if let Some(ref addr) = daemon_addr {
        let trimmed = addr.trim();
        if !trimmed.is_empty() {
            if trimmed.parse::<std::net::SocketAddr>().is_err() {
                let parts: Vec<&str> = trimmed.rsplitn(2, ':').collect();
                if parts.len() != 2 || parts[0].parse::<u16>().is_err() {
                    return Err(bad_request_error(&format!(
                        "daemon_addr must be a valid host:port, got '{}'", trimmed
                    )));
                }
            }
        }
    }

    update_config_file(config_state, subsystem, ui_tx, |raw| {
        raw.ups = Some(crate::config::RawUpsConfig {
            enabled,
            daemon_addr,
            poll_interval_secs,
            upstream_heartbeat_secs,
        });
        Ok(())
    })
    .await
}
```

- [ ] **Step 2: Add HTTP route**

In `build_router`, add:

```rust
.route("/api/v1/config/ups", post(post_config_section_handler))
```

(The generic `post_config_section_handler` should already dispatch based on the path segment; if it's per-section handlers, add a specific one following the existing pattern.)

- [ ] **Step 3: Add tests for UPS config section update**

Add tests following the pattern of existing section tests:

```rust
#[tokio::test]
async fn config_ups_section_accepts_valid_values() {
    let (config_state, subsystem, ui_tx) = test_config_setup().await;
    let payload = serde_json::json!({
        "enabled": true,
        "daemon_addr": "127.0.0.1:8423",
        "poll_interval_secs": 10,
        "upstream_heartbeat_secs": 30
    });
    let result = apply_section_update("ups", &payload, &config_state, &subsystem, &ui_tx, None).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn config_ups_section_rejects_invalid_poll_interval() {
    let (config_state, subsystem, ui_tx) = test_config_setup().await;
    let payload = serde_json::json!({ "poll_interval_secs": 0 });
    let result = apply_section_update("ups", &payload, &config_state, &subsystem, &ui_tx, None).await;
    assert!(result.is_err());
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p forwarder --lib status_http`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add services/forwarder/src/status_http.rs
git commit -m "feat(forwarder): add UPS config editing API section"
```

---

## Task 7: Server — migration, AppState cache, DashboardEvent

**Files:**
- Create: `services/server/migrations/0011_forwarder_ups_events.sql`
- Modify: `services/server/src/state.rs`
- Modify: `services/server/src/dashboard_events.rs`
- Modify: `services/server/src/http/sse.rs`

- [ ] **Step 1: Create migration**

Create `services/server/migrations/0011_forwarder_ups_events.sql`:

```sql
CREATE TABLE forwarder_ups_events (
    id              BIGSERIAL PRIMARY KEY,
    forwarder_id    TEXT NOT NULL,
    event_type      TEXT NOT NULL CHECK (event_type IN ('power_lost', 'power_restored')),
    battery_percent SMALLINT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_ups_events_forwarder ON forwarder_ups_events(forwarder_id, created_at);
```

- [ ] **Step 2: Add ForwarderUpsCache to AppState**

In `services/server/src/state.rs`, add:

```rust
#[derive(Debug, Clone)]
pub struct CachedUpsState {
    pub available: bool,
    pub status: Option<rt_protocol::UpsStatus>,
}

pub type ForwarderUpsCache = Arc<RwLock<HashMap<String, CachedUpsState>>>;
```

Add to `AppState`:

```rust
pub forwarder_ups_cache: ForwarderUpsCache,
```

Initialize in `AppState::new`:

```rust
forwarder_ups_cache: Arc::new(RwLock::new(HashMap::new())),
```

- [ ] **Step 3: Add ForwarderUpsUpdated to DashboardEvent**

In `services/server/src/dashboard_events.rs`, add to the enum:

```rust
ForwarderUpsUpdated {
    forwarder_id: String,
    available: bool,
    status: Option<rt_protocol::UpsStatus>,
},
```

Add a test:

```rust
#[test]
fn forwarder_ups_updated_serializes_with_type_tag() {
    let event = DashboardEvent::ForwarderUpsUpdated {
        forwarder_id: "fwd-1".to_owned(),
        available: true,
        status: Some(rt_protocol::UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1711929600000,
        }),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "forwarder_ups_updated");
    assert_eq!(json["forwarder_id"], "fwd-1");
    assert_eq!(json["available"], true);
    assert_eq!(json["status"]["battery_percent"], 73);
}
```

- [ ] **Step 4: Add SSE event type mapping**

In `services/server/src/http/sse.rs`, add a match arm in `dashboard_sse`:

```rust
crate::dashboard_events::DashboardEvent::ForwarderUpsUpdated { .. } => {
    "forwarder_ups_updated"
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p server`
Expected: PASS

- [ ] **Step 6: Regenerate sqlx offline cache**

Run: `cd services/server && cargo sqlx prepare`
Expected: `.sqlx/` files updated

- [ ] **Step 7: Commit**

```bash
git add services/server/migrations/0011_forwarder_ups_events.sql services/server/src/state.rs services/server/src/dashboard_events.rs services/server/src/http/sse.rs services/server/.sqlx/
git commit -m "feat(server): add UPS events migration, AppState cache, and DashboardEvent variant"
```

---

## Task 8: Server — ws_forwarder.rs handler

**Files:**
- Modify: `services/server/src/ws_forwarder.rs`

- [ ] **Step 1: Add ForwarderUpsStatus match arm**

In `ws_forwarder.rs`, in the main message dispatch (after `ReaderDownloadProgress` handler, ~line 721), add:

```rust
Ok(WsMessage::ForwarderUpsStatus(ups)) => {
    // 1. Check power_plugged transition for event logging
    let prev_plugged = {
        let cache = state.forwarder_ups_cache.read().await;
        cache.get(&device_id).and_then(|c| c.status.as_ref().map(|s| s.power_plugged))
    };

    // 2. Update cache
    {
        let mut cache = state.forwarder_ups_cache.write().await;
        cache.insert(device_id.clone(), crate::state::CachedUpsState {
            available: ups.available,
            status: ups.status.clone(),
        });
    }

    // 3. Log power_plugged transitions to DB
    if let Some(ref status) = ups.status {
        if let Some(was_plugged) = prev_plugged {
            if was_plugged && !status.power_plugged {
                // Power lost
                let _ = sqlx::query(
                    "INSERT INTO forwarder_ups_events (forwarder_id, event_type, battery_percent) VALUES ($1, 'power_lost', $2)"
                )
                .bind(&device_id)
                .bind(status.battery_percent as i16)
                .execute(&state.pool)
                .await;
            } else if !was_plugged && status.power_plugged {
                // Power restored
                let _ = sqlx::query(
                    "INSERT INTO forwarder_ups_events (forwarder_id, event_type, battery_percent) VALUES ($1, 'power_restored', $2)"
                )
                .bind(&device_id)
                .bind(status.battery_percent as i16)
                .execute(&state.pool)
                .await;
            }
        }
    }

    // 4. Emit dashboard SSE event
    let _ = state.dashboard_tx.send(DashboardEvent::ForwarderUpsUpdated {
        forwarder_id: device_id.clone(),
        available: ups.available,
        status: ups.status.clone(),
    });

    // 5. Broadcast sentinel ReadEvent on one of the forwarder's streams
    if ups.status.is_some() {
        // Pick the first stream channel for this forwarder
        if let Some(first_sid) = stream_map.values().next() {
            let sentinel_msg = WsMessage::ForwarderUpsStatus(rt_protocol::ForwarderUpsStatus {
                forwarder_id: device_id.clone(),
                available: ups.available,
                status: ups.status.clone(),
            });
            match serde_json::to_string(&sentinel_msg) {
                Ok(json) => {
                    let tx = state.get_or_create_broadcast(*first_sid).await;
                    let _ = tx.send(rt_protocol::ReadEvent {
                        forwarder_id: device_id.clone(),
                        reader_ip: String::new(),
                        stream_epoch: 0,
                        seq: 0,
                        reader_timestamp: String::new(),
                        raw_frame: json.into_bytes(),
                        read_type: rt_protocol::FORWARDER_UPS_STATUS_READ_TYPE.to_owned(),
                    });
                }
                Err(e) => {
                    error!(
                        device_id = %device_id,
                        error = %e,
                        "failed to serialize ForwarderUpsStatus for sentinel broadcast"
                    );
                }
            }
        }
    }
}
```

- [ ] **Step 2: Clear UPS cache on forwarder disconnect**

In the disconnect cleanup section (~line 860-900), add after the reader_states cleanup:

```rust
// Clear UPS cache for this forwarder
{
    state.forwarder_ups_cache.write().await.remove(&device_id);
    let _ = state.dashboard_tx.send(DashboardEvent::ForwarderUpsUpdated {
        forwarder_id: device_id.clone(),
        available: false,
        status: None,
    });
}
```

- [ ] **Step 3: Run compilation check**

Run: `cargo check -p server`
Expected: No errors

- [ ] **Step 4: Commit**

```bash
git add services/server/src/ws_forwarder.rs
git commit -m "feat(server): handle ForwarderUpsStatus — cache, dashboard SSE, sentinel broadcast, transition logging"
```

---

## Task 9: Server — receiver initial UPS snapshot + sentinel forwarding

**Files:**
- Modify: `services/server/src/ws_receiver.rs`

- [ ] **Step 1: Add __forwarder_ups_status sentinel forwarding**

In `ws_receiver.rs`, in the sentinel event dispatch block (~line 1295-1320), add a new branch after the `READER_DOWNLOAD_PROGRESS_READ_TYPE` handler:

```rust
} else if event.read_type == rt_protocol::FORWARDER_UPS_STATUS_READ_TYPE {
    match String::from_utf8(event.raw_frame) {
        Ok(json) => {
            if socket.send(Message::Text(json.into())).await.is_err() {
                warn!(
                    stream_id = %sub.stream_id,
                    "WS send failed for forwarder_ups_status; closing session"
                );
                return;
            }
        }
        Err(e) => {
            error!(
                stream_id = %sub.stream_id,
                error = %e,
                "invalid UTF-8 in forwarder_ups_status payload"
            );
        }
    }
```

- [ ] **Step 2: Send initial UPS snapshot on receiver connect**

After the initial stream metrics push loop (~line 1231), add:

```rust
// Push initial UPS status for each unique forwarder in the subscription set.
{
    let mut seen_forwarders = std::collections::HashSet::new();
    let ups_cache = state.forwarder_ups_cache.read().await;
    for target in &resolved_targets {
        if !seen_forwarders.insert(target.forwarder_id.clone()) {
            continue; // Already sent for this forwarder
        }
        if let Some(cached) = ups_cache.get(&target.forwarder_id) {
            let msg = WsMessage::ForwarderUpsStatus(rt_protocol::ForwarderUpsStatus {
                forwarder_id: target.forwarder_id.clone(),
                available: cached.available,
                status: cached.status.clone(),
            });
            if let Ok(json) = serde_json::to_string(&msg) {
                if socket.send(Message::Text(json.into())).await.is_err() {
                    warn!(
                        device_id = %device_id,
                        forwarder_id = %target.forwarder_id,
                        "failed to send initial UPS snapshot"
                    );
                }
            }
        }
    }
}
```

- [ ] **Step 3: Run compilation check**

Run: `cargo check -p server`
Expected: No errors

- [ ] **Step 4: Commit**

```bash
git add services/server/src/ws_receiver.rs
git commit -m "feat(server): forward UPS sentinel events to receivers + send initial snapshot on connect"
```

---

## Task 10: Receiver — handle ForwarderUpsStatus

**Files:**
- Modify: `services/receiver/src/session.rs`
- Modify: `services/receiver/src/ui_events.rs`

- [ ] **Step 1: Add ForwarderUpsUpdated to ReceiverUiEvent**

In `services/receiver/src/ui_events.rs`, add to the `ReceiverUiEvent` enum:

```rust
ForwarderUpsUpdated {
    forwarder_id: String,
    available: bool,
    status: Option<rt_protocol::UpsStatus>,
},
```

Add a test:

```rust
#[test]
fn forwarder_ups_updated_serializes_with_type_tag() {
    let event = ReceiverUiEvent::ForwarderUpsUpdated {
        forwarder_id: "fwd-01".to_owned(),
        available: true,
        status: Some(rt_protocol::UpsStatus {
            battery_percent: 73,
            battery_voltage_mv: 3870,
            charging: true,
            power_plugged: true,
            temperature_cdeg: 4250,
            sampled_at: 1711929600000,
        }),
    };
    let json: serde_json::Value = serde_json::to_value(&event).unwrap();
    assert_eq!(json["type"], "forwarder_ups_updated");
    assert_eq!(json["forwarder_id"], "fwd-01");
    assert_eq!(json["available"], true);
    assert_eq!(json["status"]["battery_percent"], 73);
}
```

- [ ] **Step 2: Handle ForwarderUpsStatus in receiver session**

Both sentinel events (via `raw_frame` in the per-stream broadcast) and the initial snapshot (sent directly by the server) arrive as `WsMessage::ForwarderUpsStatus` JSON. The receiver parses them uniformly as `WsMessage` variants.

In receiver `session.rs`, add after `ReceiverReaderDownloadProgress` handler:

```rust
Ok(WsMessage::ForwarderUpsStatus(ups)) => {
    if deps.ui_tx.send(crate::ui_events::ReceiverUiEvent::ForwarderUpsUpdated {
        forwarder_id: ups.forwarder_id,
        available: ups.available,
        status: ups.status,
    }).is_err() {
        warn!("ui_tx closed; forwarder UPS update dropped");
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p receiver`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add services/receiver/src/session.rs services/receiver/src/ui_events.rs
git commit -m "feat(receiver): handle ForwarderUpsStatus and emit ForwarderUpsUpdated UI event"
```

---

## Task 11: Shared UI — BatteryIndicator + LowBatteryBanner components

**Files:**
- Create: `apps/shared-ui/src/components/BatteryIndicator.svelte`
- Create: `apps/shared-ui/src/components/LowBatteryBanner.svelte`
- Modify: `apps/shared-ui/src/lib/index.ts` (export new components)

- [ ] **Step 1: Create BatteryIndicator component**

Create `apps/shared-ui/src/components/BatteryIndicator.svelte`:

```svelte
<script lang="ts">
  interface Props {
    percent: number | null;
    charging?: boolean;
    available?: boolean;
    configured?: boolean;
    compact?: boolean;
  }

  let { percent, charging = false, available = true, configured = true, compact = false }: Props = $props();

  const colorClass = $derived(
    !configured || percent == null
      ? "text-gray-400"
      : !available
        ? "text-gray-400"
        : percent > 50
          ? "text-green-500"
          : percent > 20
            ? "text-yellow-500"
            : "text-red-500"
  );

  const fillWidth = $derived(
    percent != null ? Math.max(0, Math.min(100, percent)) : 0
  );

  const label = $derived(
    !configured
      ? "—"
      : !available
        ? "UPS unavailable"
        : percent != null
          ? `${percent}%`
          : "—"
  );
</script>

{#if !configured}
  <span class="text-gray-400" title="No UPS configured">—</span>
{:else}
  <span
    class="inline-flex items-center gap-1 {colorClass}"
    title={!available ? "UPS unavailable" : `${percent ?? 0}%${charging ? " (charging)" : ""}`}
  >
    <svg
      class={compact ? "w-4 h-4" : "w-5 h-5"}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
    >
      <!-- Battery body -->
      <rect x="2" y="6" width="18" height="12" rx="2" />
      <!-- Battery terminal -->
      <path d="M22 10v4" stroke-linecap="round" />
      <!-- Fill level -->
      <rect
        x="4"
        y="8"
        width={14 * fillWidth / 100}
        height="8"
        rx="1"
        fill="currentColor"
        opacity="0.6"
      />
      <!-- Charging bolt -->
      {#if charging}
        <path d="M12 7l-2 5h4l-2 5" stroke="currentColor" stroke-width="1.5" fill="none" />
      {/if}
    </svg>
    {#if !compact}
      <span class="text-sm">{label}</span>
    {/if}
  </span>
{/if}
```

- [ ] **Step 2: Create LowBatteryBanner component**

Create `apps/shared-ui/src/components/LowBatteryBanner.svelte`:

```svelte
<script lang="ts">
  interface LowBatteryForwarder {
    name: string;
    percent: number;
  }

  interface Props {
    forwarders: LowBatteryForwarder[];
    onDismiss?: () => void;
  }

  let { forwarders, onDismiss }: Props = $props();
</script>

{#if forwarders.length > 0}
  <div class="bg-red-600 text-white px-4 py-2 flex items-center justify-between text-sm rounded">
    <span>
      Low battery:
      {#each forwarders as fwd, i}
        {fwd.name} at {fwd.percent}%{i < forwarders.length - 1 ? ", " : ""}
      {/each}
    </span>
    {#if onDismiss}
      <button
        class="ml-4 text-white/80 hover:text-white"
        onclick={onDismiss}
        aria-label="Dismiss"
      >
        ✕
      </button>
    {/if}
  </div>
{/if}
```

- [ ] **Step 3: Export from shared-ui**

In `apps/shared-ui/src/lib/index.ts`, add:

```typescript
export { default as BatteryIndicator } from "../components/BatteryIndicator.svelte";
export { default as LowBatteryBanner } from "../components/LowBatteryBanner.svelte";
```

- [ ] **Step 4: Commit**

```bash
git add apps/shared-ui/src/components/BatteryIndicator.svelte apps/shared-ui/src/components/LowBatteryBanner.svelte apps/shared-ui/src/lib/index.ts
git commit -m "feat(shared-ui): add BatteryIndicator and LowBatteryBanner components"
```

---

## Task 12: Receiver UI — store + SSE + types

**Files:**
- Modify: `apps/receiver-ui/src/lib/api.ts`
- Modify: `apps/receiver-ui/src/lib/sse.ts`
- Modify: `apps/receiver-ui/src/lib/store.svelte.ts`

- [ ] **Step 1: Add UPS types to api.ts**

In `apps/receiver-ui/src/lib/api.ts`, add after the `ForwarderMetricsUpdate` interface:

```typescript
export interface UpsStatus {
  battery_percent: number;
  battery_voltage_mv: number;
  charging: boolean;
  power_plugged: boolean;
  temperature_cdeg: number;
  sampled_at: number;
}

export interface ForwarderUpsState {
  available: boolean;
  status: UpsStatus | null;
}
```

- [ ] **Step 2: Add SSE callback for UPS updates**

In `apps/receiver-ui/src/lib/sse.ts`, add the payload type:

```typescript
export type ForwarderUpsUpdatedPayload = {
  forwarder_id: string;
  available: boolean;
  status: UpsStatus | null;
};
```

(Import `UpsStatus` from `./api`)

Add to `SseCallbacks`:
```typescript
onForwarderUpsUpdated?: (payload: ForwarderUpsUpdatedPayload) => void;
```

Add listener in `initSSE`:
```typescript
listen<ForwarderUpsUpdatedPayload>("forwarder_ups_updated", (event) => {
  callbacks.onForwarderUpsUpdated?.(event.payload);
}),
```

- [ ] **Step 3: Add upsState to store and wire callback**

In `apps/receiver-ui/src/lib/store.svelte.ts`, add to the store state:

```typescript
upsState: new Map<string, { available: boolean; status: api.UpsStatus | null }>(),
```

Add an update function:
```typescript
function applyForwarderUpsUpdate(forwarderId: string, available: boolean, status: api.UpsStatus | null): void {
  const next = new Map(store.upsState);
  if (!available && status === null) {
    next.delete(forwarderId); // Forwarder disconnected, clear state
  } else {
    next.set(forwarderId, { available, status });
  }
  store.upsState = next;
}
```

Wire in `initSSE` callbacks:
```typescript
onForwarderUpsUpdated: (payload) => {
  applyForwarderUpsUpdate(payload.forwarder_id, payload.available, payload.status);
},
```

- [ ] **Step 4: Run vitest**

Run: `cd apps/receiver-ui && npx vitest run`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/receiver-ui/src/lib/api.ts apps/receiver-ui/src/lib/sse.ts apps/receiver-ui/src/lib/store.svelte.ts
git commit -m "feat(receiver-ui): add UPS state types, SSE handler, and store"
```

---

## Task 13: Receiver UI — ForwardersTab battery display

**Files:**
- Modify: `apps/receiver-ui/src/lib/components/ForwardersTab.svelte`

- [ ] **Step 1: Add battery column to list view**

Import the shared components:
```svelte
import { BatteryIndicator, LowBatteryBanner } from "@rusty-timer/shared-ui";
```

Import store UPS state:
```svelte
import { store } from "$lib/store.svelte";
```

Add battery column header between Name and Readers in the table:
```svelte
<th class="...">Battery</th>
```

Add battery cell in the row:
```svelte
<td class="...">
  {@const upsEntry = store.upsState.get(fwd.forwarder_id)}
  <BatteryIndicator
    percent={upsEntry?.status?.battery_percent ?? null}
    charging={upsEntry?.status?.charging ?? false}
    available={upsEntry?.available ?? true}
    configured={upsEntry !== undefined}
    compact
  />
</td>
```

- [ ] **Step 2: Add battery stats card to detail view**

Add a 5th card in the detail grid (change `grid-cols-4` to `grid-cols-5` or use responsive wrapping):

```svelte
{@const upsEntry = store.upsState.get(fwd.forwarder_id)}
{#if upsEntry}
  <Card>
    <span class="text-xs text-gray-500 uppercase tracking-wide">Battery</span>
    <div class="flex items-center gap-2">
      <BatteryIndicator
        percent={upsEntry.status?.battery_percent ?? null}
        charging={upsEntry.status?.charging ?? false}
        available={upsEntry.available}
        configured
      />
    </div>
    {#if upsEntry.status}
      <div class="text-xs text-gray-500 mt-1">
        {(upsEntry.status.battery_voltage_mv / 1000).toFixed(2)}V
        / {(upsEntry.status.temperature_cdeg / 100).toFixed(1)}°C
      </div>
      <div class="text-xs mt-1">
        {#if upsEntry.status.charging}
          Charging
        {:else if upsEntry.status.power_plugged}
          Plugged In
        {:else}
          On Battery
        {/if}
      </div>
      <div class="text-xs text-gray-400 mt-1">
        Updated {Math.round((Date.now() - upsEntry.status.sampled_at) / 1000)}s ago
      </div>
    {:else if !upsEntry.available}
      <div class="text-xs text-gray-400 mt-1">UPS unavailable</div>
    {/if}
  </Card>
{/if}
```

- [ ] **Step 3: Add low battery warning banner**

At the top of the component (above the list/detail view):

```svelte
{@const lowBatteryForwarders = (store.forwarders ?? [])
  .filter(f => {
    const ups = store.upsState.get(f.forwarder_id);
    return ups?.status && ups.status.battery_percent <= 15 && !ups.status.power_plugged;
  })
  .map(f => ({
    name: f.display_name ?? f.forwarder_id,
    percent: store.upsState.get(f.forwarder_id)!.status!.battery_percent,
  }))}

<LowBatteryBanner
  forwarders={lowBatteryForwarders}
  onDismiss={() => { /* optional dismiss state */ }}
/>
```

- [ ] **Step 4: Commit**

```bash
git add apps/receiver-ui/src/lib/components/ForwardersTab.svelte
git commit -m "feat(receiver-ui): add battery column, detail card, and low battery banner to ForwardersTab"
```

---

## Task 14: Receiver UI — StreamsTab battery icon

**Files:**
- Modify: `apps/receiver-ui/src/lib/components/StreamsTab.svelte`

- [ ] **Step 1: Add battery icon next to forwarder name in stream rows**

Import the component and store:
```svelte
import { BatteryIndicator } from "@rusty-timer/shared-ui";
import { store } from "$lib/store.svelte";
```

In each stream row, next to the forwarder name/status dot, add:

```svelte
{@const upsEntry = store.upsState.get(stream.forwarder_id)}
{#if upsEntry}
  <BatteryIndicator
    percent={upsEntry.status?.battery_percent ?? null}
    charging={upsEntry.status?.charging ?? false}
    available={upsEntry.available}
    configured
    compact
  />
{/if}
```

- [ ] **Step 2: Commit**

```bash
git add apps/receiver-ui/src/lib/components/StreamsTab.svelte
git commit -m "feat(receiver-ui): add battery icon to StreamsTab stream rows"
```

---

## Task 15: Dashboard (server-ui) — UPS SSE + store + display

**Files:**
- Modify: `apps/server-ui/src/lib/sse.ts`
- Modify: `apps/server-ui/src/lib/stores.ts`
- Modify: `apps/server-ui/src/routes/+page.svelte`

- [ ] **Step 1: Add UPS store**

In `apps/server-ui/src/lib/stores.ts`, add:

```typescript
import { writable } from "svelte/store";

export const upsStateStore = writable<Record<string, { available: boolean; status: any | null }>>({});

export function setUpsState(forwarderId: string, available: boolean, status: any | null): void {
  upsStateStore.update((current) => {
    const next = { ...current };
    if (!available && status === null) {
      delete next[forwarderId];
    } else {
      next[forwarderId] = { available, status };
    }
    return next;
  });
}
```

- [ ] **Step 2: Add SSE listener**

In `apps/server-ui/src/lib/sse.ts`, add:

```typescript
import { setUpsState } from "./stores";
```

Add listener in `initSSE`:
```typescript
eventSource.addEventListener("forwarder_ups_updated", (e: MessageEvent) => {
  try {
    const data = JSON.parse(e.data);
    setUpsState(data.forwarder_id, data.available, data.status ?? null);
  } catch (err) {
    console.error("Failed to parse forwarder_ups_updated event:", err);
  }
});
```

- [ ] **Step 3: Add battery display to dashboard forwarders page**

In `apps/server-ui/src/routes/+page.svelte`, import:

```svelte
<script>
  import { BatteryIndicator, LowBatteryBanner } from "@rusty-timer/shared-ui";
  import { upsStateStore } from "$lib/stores";
</script>
```

At the top of the page content (before the streams/forwarder groups), add the low battery banner:

```svelte
{@const lowBatteryForwarders = Object.entries($upsStateStore)
  .filter(([_, ups]) => ups.status && ups.status.battery_percent <= 15 && !ups.status.power_plugged)
  .map(([fwdId, ups]) => ({
    name: fwdId,
    percent: ups.status!.battery_percent,
  }))}

<LowBatteryBanner forwarders={lowBatteryForwarders} />
```

In each forwarder group header (next to the forwarder name), add:

```svelte
{@const upsEntry = $upsStateStore[forwarderId]}
{#if upsEntry}
  <BatteryIndicator
    percent={upsEntry.status?.battery_percent ?? null}
    charging={upsEntry.status?.charging ?? false}
    available={upsEntry.available}
    configured
    compact
  />
{/if}
```

- [ ] **Step 4: Commit**

```bash
git add apps/server-ui/src/lib/sse.ts apps/server-ui/src/lib/stores.ts apps/server-ui/src/routes/+page.svelte
git commit -m "feat(server-ui): add UPS state store, SSE handler, and battery display on dashboard"
```

---

## Task 16: Forwarder local UI — battery indicator

**Files:**
- Modify: `apps/forwarder-ui/src/routes/+page.svelte`
- Modify: `apps/forwarder-ui/src/lib/sse.ts`

- [ ] **Step 1: Add UPS SSE listener to forwarder UI**

In `apps/forwarder-ui/src/lib/sse.ts`, add a listener for the `ups_status_changed` event (matching the `ForwarderUiEvent::UpsStatusChanged` variant):

```typescript
listen("ups_status_changed", (event) => {
  callbacks.onUpsStatusChanged?.(event.payload);
}),
```

Add the callback type to the callbacks interface.

- [ ] **Step 2: Add battery indicator to status bar**

In `apps/forwarder-ui/src/routes/+page.svelte`, add a battery indicator in the status area (alongside the ready/uplink status badges):

```svelte
import { BatteryIndicator } from "@rusty-timer/shared-ui";

let upsState = $state<{ available: boolean; status: any | null } | null>(null);
```

Wire the SSE callback:
```typescript
onUpsStatusChanged: (payload) => {
  upsState = { available: payload.available, status: payload.status };
},
```

Add to the template near the status badges:
```svelte
{#if upsState}
  <BatteryIndicator
    percent={upsState.status?.battery_percent ?? null}
    charging={upsState.status?.charging ?? false}
    available={upsState.available}
    configured
  />
  {#if upsState.status && !upsState.status.power_plugged}
    <AlertBanner variant="warning">Running on battery power ({upsState.status.battery_percent}%)</AlertBanner>
  {/if}
  {#if !upsState.available}
    <AlertBanner variant="warning">UPS unavailable</AlertBanner>
  {/if}
{/if}
```

- [ ] **Step 3: Commit**

```bash
git add apps/forwarder-ui/src/routes/+page.svelte apps/forwarder-ui/src/lib/sse.ts
git commit -m "feat(forwarder-ui): add battery indicator and power warnings to status area"
```

---

## Task 17: Shared UI — ForwarderConfig UPS section

**Files:**
- Modify: `apps/shared-ui/src/lib/forwarder-config-layout.ts`

- [ ] **Step 1: Add UPS section to config layout**

In the config layout definition, add a new section:

```typescript
{
  key: "ups",
  label: "UPS (PiSugar)",
  fields: [
    { key: "enabled", label: "Enabled", type: "toggle" },
    { key: "daemon_addr", label: "Daemon Address", type: "text", placeholder: "127.0.0.1:8423" },
    { key: "poll_interval_secs", label: "Poll Interval (seconds)", type: "number", min: 1, max: 60 },
    { key: "upstream_heartbeat_secs", label: "Heartbeat Interval (seconds)", type: "number", min: 10, max: 300 },
  ],
}
```

- [ ] **Step 2: Commit**

```bash
git add apps/shared-ui/src/lib/forwarder-config-layout.ts
git commit -m "feat(shared-ui): add UPS section to ForwarderConfig layout"
```

---

## Task 18: SBC setup script — PiSugar support

**Files:**
- Modify: `deploy/sbc/rt-setup.sh`

- [ ] **Step 1: Add UPS setup section**

At the top of the script, add the version variable:
```bash
PISUGAR_SERVER_VERSION="1.7.8"
```

After the existing setup sections (forwarder install + config), add:

```bash
# ---------------------------------------------------------------------------
# PiSugar UPS setup
# ---------------------------------------------------------------------------

setup_ups() {
  local ups_enabled="0"
  if is_noninteractive_mode; then
    ups_enabled="$(bool_env_is_true "${RT_SETUP_UPS_ENABLED:-0}")"
  else
    read -rp "Do you have a PiSugar UPS HAT installed? [y/N] " answer
    if [[ "${answer}" =~ ^[Yy]$ ]]; then
      ups_enabled="1"
    fi
  fi

  if [[ "${ups_enabled}" != "1" ]]; then
    log "Skipping PiSugar UPS setup"
    return
  fi

  log "Setting up PiSugar UPS support..."

  # 1. Enable I2C
  if ! grep -q "^dtparam=i2c_arm=on" /boot/config.txt 2>/dev/null; then
    echo "dtparam=i2c_arm=on" >> /boot/config.txt
    log "Enabled I2C in /boot/config.txt"
  else
    log "I2C already enabled"
  fi

  # 2. Install pisugar-server
  local arch
  arch="$(detect_arch)"
  local deb_arch
  case "${arch}" in
    aarch64-unknown-linux-gnu) deb_arch="arm64" ;;
    armv7-unknown-linux-gnueabihf) deb_arch="armhf" ;;
    *) log "ERROR: unsupported architecture for pisugar-server: ${arch}"; return 1 ;;
  esac

  local deb_url="https://github.com/PiSugar/PiSugar/releases/download/v${PISUGAR_SERVER_VERSION}/pisugar-server_${PISUGAR_SERVER_VERSION}_${deb_arch}.deb"
  local deb_file="/tmp/pisugar-server_${PISUGAR_SERVER_VERSION}_${deb_arch}.deb"

  log "Downloading pisugar-server v${PISUGAR_SERVER_VERSION}..."
  curl -fsSL -o "${deb_file}" "${deb_url}"
  dpkg -i "${deb_file}" || apt-get install -f -y
  rm -f "${deb_file}"

  # 3. Configure pisugar-server
  mkdir -p /etc/pisugar-server
  cat > /etc/pisugar-server/config.json <<'PISUGAR_EOF'
{
  "safe_shutdown_level": 10,
  "safe_shutdown_delay": 30,
  "auto_power_on": true,
  "soft_poweroff": true,
  "soft_poweroff_shell": "shutdown --poweroff 0"
}
PISUGAR_EOF
  log "Wrote pisugar-server config"

  # 4. Enable and start pisugar-server
  systemctl enable pisugar-server.service
  systemctl start pisugar-server.service || true
  log "pisugar-server service enabled and started"

  # 5. Add systemd ordering
  local service_file="/etc/systemd/system/rt-forwarder.service"
  if [[ -f "${service_file}" ]]; then
    if ! grep -q "After=pisugar-server.service" "${service_file}"; then
      sed -i '/^\[Unit\]/a After=pisugar-server.service' "${service_file}"
      systemctl daemon-reload
      log "Added After=pisugar-server.service to rt-forwarder.service"
    fi
  fi

  # 6. Update forwarder.toml
  local config_file="${CONFIG_DIR}/forwarder.toml"
  if [[ -f "${config_file}" ]]; then
    if ! grep -q '^\[ups\]' "${config_file}"; then
      cat >> "${config_file}" <<'UPS_EOF'

[ups]
enabled = true
UPS_EOF
      log "Added [ups] section to forwarder.toml"
    fi
  fi

  log "PiSugar UPS setup complete"
}
```

Call `setup_ups` at the end of the main setup flow.

- [ ] **Step 2: Commit**

```bash
git add deploy/sbc/rt-setup.sh
git commit -m "feat(sbc): add PiSugar UPS setup to rt-setup.sh with interactive/noninteractive support"
```

---

## Task 19: Cloud-init generator + dashboard SBC setup UI

**Files:**
- Modify: `scripts/sbc_cloud_init.py`
- Modify: `apps/server-ui/src/lib/sbc-setup/generate.ts`
- Modify: `apps/server-ui/src/routes/sbc-setup/+page.svelte`

- [ ] **Step 1: Add ups_enabled to Python cloud-init generator**

In `scripts/sbc_cloud_init.py`, add `ups_enabled: bool = False` to the config dataclass/arguments.

When `ups_enabled` is true:
- Add `RT_SETUP_UPS_ENABLED=1` to the generated env file
- Add `i2c-tools` to the cloud-init package list

- [ ] **Step 2: Add ups_enabled to TypeScript cloud-init generator**

In `apps/server-ui/src/lib/sbc-setup/generate.ts`, add `upsEnabled: boolean` to the config interface. When true, add `RT_SETUP_UPS_ENABLED=1` to the runcmd env vars and `i2c-tools` to the packages list.

- [ ] **Step 3: Add UPS toggle to SBC setup page**

In `apps/server-ui/src/routes/sbc-setup/+page.svelte`, add to the form data:

```typescript
upsEnabled: false,
```

Add a toggle in the form:
```svelte
<label class="flex items-center gap-2">
  <input type="checkbox" bind:checked={formData.upsEnabled} />
  <span>PiSugar UPS</span>
</label>
{#if formData.upsEnabled}
  <p class="text-xs text-gray-500 ml-6">
    Installs pisugar-server and configures safe shutdown. Requires PiSugar 3 HAT.
  </p>
{/if}
```

Pass `upsEnabled` to the generate function.

- [ ] **Step 4: Add tests for Python generator**

In `scripts/tests/test_sbc_cloud_init.py`, add:

```python
def test_ups_enabled_adds_env_var_and_package():
    result = generate_cloud_init(ups_enabled=True, ...)
    assert "RT_SETUP_UPS_ENABLED=1" in result.env_content
    assert "i2c-tools" in result.packages
```

- [ ] **Step 5: Run Python tests**

Run: `cd scripts && python -m pytest tests/test_sbc_cloud_init.py -v`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add scripts/sbc_cloud_init.py scripts/tests/test_sbc_cloud_init.py apps/server-ui/src/lib/sbc-setup/generate.ts apps/server-ui/src/routes/sbc-setup/+page.svelte
git commit -m "feat(sbc): add UPS toggle to cloud-init generator and dashboard SBC setup page"
```

---

## Task 20: Vitest UI tests

**Files:**
- Create: `apps/receiver-ui/src/lib/components/BatteryIndicator.test.ts`
- Modify: `apps/receiver-ui/src/lib/components/ForwardersTab.test.ts` (or create if absent)

- [ ] **Step 1: Write vitest tests for BatteryIndicator**

Create `apps/receiver-ui/src/lib/components/BatteryIndicator.test.ts`:

```typescript
import { render } from "@testing-library/svelte";
import { describe, it, expect } from "vitest";
import BatteryIndicator from "@rusty-timer/shared-ui/components/BatteryIndicator.svelte";

describe("BatteryIndicator", () => {
  it("renders dash when not configured", () => {
    const { getByText } = render(BatteryIndicator, { percent: null, configured: false });
    expect(getByText("—")).toBeTruthy();
  });

  it("renders UPS unavailable when available is false", () => {
    const { container } = render(BatteryIndicator, { percent: null, available: false, configured: true });
    expect(container.querySelector("[title='UPS unavailable']")).toBeTruthy();
  });

  it("renders green color class when percent > 50", () => {
    const { container } = render(BatteryIndicator, { percent: 73, configured: true, available: true });
    expect(container.querySelector(".text-green-500")).toBeTruthy();
  });

  it("renders yellow color class when percent is 20-50", () => {
    const { container } = render(BatteryIndicator, { percent: 35, configured: true, available: true });
    expect(container.querySelector(".text-yellow-500")).toBeTruthy();
  });

  it("renders red color class when percent < 20", () => {
    const { container } = render(BatteryIndicator, { percent: 10, configured: true, available: true });
    expect(container.querySelector(".text-red-500")).toBeTruthy();
  });

  it("shows charging bolt when charging", () => {
    const { container } = render(BatteryIndicator, { percent: 50, charging: true, configured: true, available: true });
    // The charging bolt is a path element inside the SVG
    const paths = container.querySelectorAll("svg path");
    expect(paths.length).toBeGreaterThan(1); // terminal + bolt
  });
});
```

- [ ] **Step 2: Write vitest tests for LowBatteryBanner**

Add to same test file or create `apps/receiver-ui/src/lib/components/LowBatteryBanner.test.ts`:

```typescript
import { render } from "@testing-library/svelte";
import { describe, it, expect } from "vitest";
import LowBatteryBanner from "@rusty-timer/shared-ui/components/LowBatteryBanner.svelte";

describe("LowBatteryBanner", () => {
  it("renders nothing when no low-battery forwarders", () => {
    const { container } = render(LowBatteryBanner, { forwarders: [] });
    expect(container.textContent?.trim()).toBe("");
  });

  it("shows warning for one low-battery forwarder", () => {
    const { getByText } = render(LowBatteryBanner, {
      forwarders: [{ name: "Start Line", percent: 12 }],
    });
    expect(getByText(/Start Line at 12%/)).toBeTruthy();
  });

  it("shows multiple low-battery forwarders together", () => {
    const { getByText } = render(LowBatteryBanner, {
      forwarders: [
        { name: "Start", percent: 10 },
        { name: "Finish", percent: 8 },
      ],
    });
    expect(getByText(/Start at 10%/)).toBeTruthy();
    expect(getByText(/Finish at 8%/)).toBeTruthy();
  });
});
```

- [ ] **Step 3: Run vitest**

Run: `cd apps/receiver-ui && npx vitest run`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add apps/receiver-ui/src/lib/components/BatteryIndicator.test.ts apps/receiver-ui/src/lib/components/LowBatteryBanner.test.ts
git commit -m "test(receiver-ui): add vitest tests for BatteryIndicator and LowBatteryBanner"
```

---

## Task 21: Forwarder integration tests

**Files:**
- Modify: existing forwarder integration test file (find via `tests/` in forwarder crate or `tests/integration/`)

- [ ] **Step 1: Test UPS config absent — no task spawned**

Write an integration test that starts a forwarder with no `[ups]` section in the TOML config. Assert:
- The `/api/v1/status` JSON response does NOT contain `ups_status` (or it is `null`)
- No `UpsStatusChanged` SSE events are emitted

```rust
#[tokio::test]
async fn ups_config_absent_no_ups_in_status() {
    // Start forwarder with minimal config (no [ups] section)
    let forwarder = start_test_forwarder(minimal_config()).await;
    let status: serde_json::Value = forwarder.get_status().await;
    assert!(status.get("ups_status").is_none() || status["ups_status"].is_null());
}
```

- [ ] **Step 2: Test UPS enabled, daemon unreachable — graceful degradation**

Write an integration test that starts a forwarder with `[ups] enabled = true` pointing to a port nothing listens on. Assert:
- The `/api/v1/status` JSON contains `ups_status` with `available: false`
- Warn logs are emitted (check log output)

```rust
#[tokio::test]
async fn ups_enabled_daemon_unreachable_graceful_degradation() {
    let config = minimal_config_with_ups("127.0.0.1:1"); // nothing listening
    let forwarder = start_test_forwarder(config).await;
    // Wait for at least one poll cycle
    tokio::time::sleep(Duration::from_secs(6)).await;
    let status: serde_json::Value = forwarder.get_status().await;
    let ups = &status["ups_status"];
    assert_eq!(ups["available"], false);
}
```

- [ ] **Step 3: Test UPS enabled, daemon available — live status**

Write an integration test using the mock TCP daemon from `pisugar_client.rs` tests:

```rust
#[tokio::test]
async fn ups_enabled_daemon_available_status_populated() {
    let mock_addr = start_mock_pisugar_daemon(73, "3.87", true, true, "42.5").await;
    let config = minimal_config_with_ups(&mock_addr);
    let forwarder = start_test_forwarder(config).await;
    // Wait for at least one poll cycle
    tokio::time::sleep(Duration::from_secs(6)).await;
    let status: serde_json::Value = forwarder.get_status().await;
    let ups = &status["ups_status"];
    assert_eq!(ups["available"], true);
    assert_eq!(ups["status"]["battery_percent"], 73);
    assert!(ups["status"]["sampled_at"].as_i64().unwrap() > 0);
}
```

- [ ] **Step 4: Run integration tests**

Run: `cargo test -p forwarder --test '*'` (or the appropriate integration test command)
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add services/forwarder/tests/
git commit -m "test(forwarder): add UPS integration tests for absent, unreachable, and available scenarios"
```

---

## Task 22: Final verification

- [ ] **Step 1: Run all Rust tests**

Run: `cargo test --workspace`
Expected: PASS

- [ ] **Step 2: Run all vitest tests**

Run: `cd apps/receiver-ui && npx vitest run && cd ../server-ui && npx vitest run`
Expected: PASS

- [ ] **Step 3: Run cargo clippy**

Run: `cargo clippy --workspace`
Expected: No warnings

- [ ] **Step 4: Verify sqlx offline cache is current**

Run: `cd services/server && cargo sqlx prepare --check`
Expected: PASS

- [ ] **Step 5: Final commit (if any remaining changes)**

```bash
git status
```
