# Forwarder

The forwarder reads chip-read data from IPICO timing hardware over TCP,
journals every event to local SQLite for power-loss safety, serves typed P2P
control/data streams over iroh to allowed receivers, and can expose an embedded
local status/configuration UI.

## Build

```bash
cargo build --release -p forwarder
```

To include the embedded web UI in the binary:

```bash
cargo build --release -p forwarder --features embed-ui
```

Forwarder release builds for SBCs also enable display support with the `lcd`
feature.

## Configuration

The forwarder is configured entirely via TOML. No environment variable overrides
are supported for config fields.

### Top-level fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `schema_version` | `u32` | Yes | -- | Must be `1`. |
| `display_name` | `String` | No | -- | Human-readable name such as `Start Line`. |

### `[auth]`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `token_file` | `String` | Yes | File containing the bootstrap/enrollment bearer token used for server registration. |

### `[journal]`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `sqlite_path` | `String` | No | `/var/lib/rusty-timer/forwarder.sqlite3` | SQLite journal path. |
| `prune_watermark_pct` | `u8` | No | `80` | Disk-usage percentage at which old events may be pruned. |
| `min_retention` | duration string | No | `7d` | Minimum event age to retain before normal pruning. |
| `max_retention` | duration string | No | `30d` | Maximum event age retained before normal pruning. |
| `emergency_free_disk_bytes` | `u64` | No | `1000000000` | Free-disk target for emergency pruning. |
| `emergency_max_rows` | `i64` | No | `1000000` | Maximum rows retained during emergency pruning. |

### `[p2p]`

P2P is disabled unless `enabled = true`. A P2P-enabled forwarder must have at
least one allow-list source: `static_allowed_receivers`, `allowlist_cache_path`,
or `server_url`.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `enabled` | `bool` | No | `false` | Starts the iroh P2P endpoint when true. |
| `secret_key_path` | `String` | No | `/var/lib/rusty-timer/p2p-secret.key` | Stable iroh endpoint key path. Mutually exclusive with `secret_key_seed_hex`. |
| `secret_key_seed_hex` | `String` | No | -- | Deterministic 32-byte hex seed for loopback tests. |
| `bind_addr_v4` | `String` | No | `0.0.0.0:0` | UDP bind address for the iroh endpoint. |
| `relay_disabled` | `bool` | No | `false` | Disable iroh relay use; used by deterministic loopback tests. |
| `discovery_disabled` | `bool` | No | `false` | Disable iroh discovery; used by deterministic loopback tests. |
| `max_concurrent_bidi_streams` | `u32` | No | iroh default | Optional iroh stream limit; must be at least `2` when P2P is enabled. |
| `static_allowed_receivers` | `Vec<String>` | No | `[]` | Receiver node IDs allowed without server allow-list polling. |
| `allowlist_cache_path` | `String` | No | -- | Last-known receiver allow-list cache. |
| `server_url` | `String` | No | -- | Coordination server URL for registration, catalog updates, and allow-list fetches. |
| `server_token_file` | `String` | No | -- | File containing the enrollment voucher used to mint a per-device server token. Required when `server_url` is set. |
| `device_token_file` | `String` | No | sibling `p2p-device-token` next to the secret key | Writable cache for the minted per-device server token. |
| `allowlist_poll_interval_secs` | `u64` | No | `60` | Server allow-list poll interval. |
| `allowlist_request_timeout_secs` | `u64` | No | `10` | Server allow-list request timeout. |

### `[status_http]`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `bind` | `String` | No | `127.0.0.1:8080` | Status HTTP bind address. SBC setup commonly writes `0.0.0.0:80`. |

### `[control]`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `allow_power_actions` | `bool` | No | `false` | Enables local control API actions that restart or shut down the host. |
| `allow_remote_config` | `bool` | No | `true` | Allows receivers to read/write forwarder config over the P2P control plane. |
| `allow_reader_control` | `bool` | No | `true` | Allows receivers to execute reader-control verbs (status, download, clear, epoch) over the P2P control plane. |

### `[update]`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `mode` | `String` | No | `check-and-download` | One of `disabled`, `check-only`, or `check-and-download`. |

### `[ups]`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `enabled` | `bool` | No | `false` | Enables polling the local UPS daemon. |
| `daemon_addr` | `String` | No | `127.0.0.1:8423` | UPS daemon host and port. |
| `poll_interval_secs` | `u64` | No | `5` | Local UPS poll interval; must be `1` through `60`. |
| `upstream_heartbeat_secs` | `u64` | No | `60` | Minimum upstream heartbeat interval; must be `10` through `300`. |

### `[[readers]]`

At least one reader entry is required.

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `target` | `String` | Yes | -- | Reader endpoint target: `A.B.C.D:PORT` or last-octet range `A.B.C.START-END:PORT`. |
| `enabled` | `bool` | No | `true` | Set to `false` to skip this reader. |
| `local_fallback_port` | `u16` | No | `10000 + last_octet` | Local TCP fanout listener for this reader. |

## Usage

```bash
forwarder --config <path>
```

The `--config` flag defaults to `/etc/rusty-timer/forwarder.toml`.

## Example config

```toml
schema_version = 1
display_name = "Start Line"

[auth]
token_file = "/etc/rusty-timer/forwarder.token"

[journal]
sqlite_path = "/var/lib/rusty-timer/forwarder.sqlite3"
prune_watermark_pct = 80
min_retention = "7d"
max_retention = "30d"

[p2p]
enabled = true
secret_key_path = "/var/lib/rusty-timer/forwarder-p2p.key"
server_url = "https://server.example.com"
server_token_file = "/etc/rusty-timer/forwarder.token"
device_token_file = "/var/lib/rusty-timer/forwarder-device.token"
allowlist_cache_path = "/var/lib/rusty-timer/receiver-allowlist.json"

[status_http]
bind = "127.0.0.1:8080"

[control]
allow_power_actions = false
allow_remote_config = true
allow_reader_control = true

[update]
mode = "check-and-download"

[[readers]]
target = "192.168.1.50:10000"
enabled = true
```

## Operations

See [Forwarder operations](../../docs/runbooks/forwarder-operations.md) for
startup, recovery, retention, and epoch procedures.
