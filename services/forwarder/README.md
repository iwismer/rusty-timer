# Forwarder

The forwarder reads chip-read data from IPICO timing hardware over TCP,
journals every event to a local SQLite database for power-loss safety, and
forwards events to the rusty-timer server over a WebSocket connection with
at-least-once delivery. An embedded web UI (opt-in at build time) provides
real-time status and configuration.

## Build

```bash
cargo build --release -p forwarder
```

To include the embedded web UI in the binary:

```bash
cargo build --release -p forwarder --features embed-ui
```

## Configuration

The forwarder is configured entirely via a TOML file. No environment variable
overrides are supported for config fields.

### Top-level fields

| Field              | Type            | Required | Default | Description                                        |
| ------------------ | --------------- | -------- | ------- | -------------------------------------------------- |
| `schema_version`   | `u32`           | Yes      | --      | Must be `1`.                                       |
| `display_name`     | `String`        | No       | --      | Human-readable name for this forwarder (e.g. "Start Line"). |

### `[server]`

| Field               | Type     | Required | Default                  | Description                            |
| -------------------- | -------- | -------- | ------------------------ | -------------------------------------- |
| `base_url`           | `String` | Yes      | --                       | Server URL (e.g. `https://example.com`). |
| `forwarders_ws_path` | `String` | No       | `/ws/v1/forwarders`      | WebSocket endpoint path.               |

### `[auth]`

| Field        | Type     | Required | Default | Description                                              |
| ------------ | -------- | -------- | ------- | -------------------------------------------------------- |
| `token_file` | `String` | Yes      | --      | Path to a file containing the bearer token (single line, trimmed on read). |

### `[journal]`

| Field                | Type   | Required | Default                                    | Description                                      |
| -------------------- | ------ | -------- | ------------------------------------------ | ------------------------------------------------ |
| `sqlite_path`        | `String` | No    | `/var/lib/rusty-timer/forwarder.sqlite3`   | Path to the SQLite journal database.             |
| `prune_watermark_pct`| `u8`   | No       | `80`                                       | Disk-usage percentage at which old events are pruned. |

### `[status_http]`

| Field  | Type     | Required | Default        | Description                              |
| ------ | -------- | -------- | -------------- | ---------------------------------------- |
| `bind` | `String` | No       | `0.0.0.0:8080` | Address and port for the status HTTP server. |

### `[uplink]`

| Field              | Type     | Required | Default       | Description                                          |
| ------------------ | -------- | -------- | ------------- | ---------------------------------------------------- |
| `batch_mode`       | `String` | No       | `immediate`   | Batching strategy for event delivery.                |
| `batch_flush_ms`   | `u64`    | No       | `100`         | Flush interval in milliseconds between batch sends.  |
| `batch_max_events` | `u32`    | No       | `50`          | Maximum number of events per batch.                  |

### `[[readers]]`

At least one `[[readers]]` entry is required.

| Field                | Type     | Required | Default                          | Description                                                |
| -------------------- | -------- | -------- | -------------------------------- | ---------------------------------------------------------- |
| `target`             | `String` | Yes      | --                               | Reader endpoint target: single `A.B.C.D:PORT` or last-octet range `A.B.C.START-END:PORT` (CIDR is not supported). |
| `enabled`            | `bool`   | No       | `true`                           | Set to `false` to skip this reader.                        |
| `local_fallback_port`| `u16`    | No       | `10000 + last_octet` of reader IP | Local TCP port for the fanout listener for this reader.   |

## Usage

```bash
forwarder --config <path>
```

The `--config` flag specifies the path to the TOML configuration file. When
omitted it defaults to `/etc/rusty-timer/forwarder.toml`.

### Logging

The forwarder uses the `RUST_LOG` environment variable to control log verbosity
(via `tracing-subscriber` with `EnvFilter`). The default level is `info`.

```bash
RUST_LOG=debug forwarder --config ./forwarder.toml
```

## Example config

```toml
schema_version = 1
display_name = "Start Line"

[server]
base_url = "https://timing.example.com"
# forwarders_ws_path = "/ws/v1/forwarders"  # default

[auth]
token_file = "/etc/rusty-timer/token"

[journal]
sqlite_path = "/var/lib/rusty-timer/forwarder.sqlite3"
prune_watermark_pct = 80

[status_http]
bind = "0.0.0.0:8080"

[uplink]
batch_mode = "immediate"
batch_flush_ms = 100
batch_max_events = 50

[[readers]]
target = "10.0.0.1"
enabled = true
# local_fallback_port = 10001  # auto-derived from last octet

[[readers]]
target = "10.0.0.2"
enabled = true
```

## Deployment

The forwarder is designed to run on a single-board computer (SBC) co-located
with the IPICO readers. See [`deploy/sbc/`](../../deploy/sbc/) for provisioning
scripts and network configuration.
