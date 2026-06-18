# Forwarder

The forwarder reads chip-read data from IPICO timing hardware over TCP,
journals every event to local SQLite for power-loss safety, and serves typed
P2P control/data streams over iroh to allowed receivers. An embedded web UI
(opt-in at build time) provides local status and configuration.

## Build

```bash
cargo build --release -p forwarder
```

To include the embedded web UI in the binary:

```bash
cargo build --release -p forwarder --features embed-ui
```

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
| `token_file` | `String` | Yes | File containing the bearer token used for thin-node registration and allow-list fetches. |

### `[journal]`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `sqlite_path` | `String` | No | `/var/lib/rusty-timer/forwarder.sqlite3` | SQLite journal path. |
| `prune_watermark_pct` | `u8` | No | `80` | Disk-usage percentage at which old events may be pruned. |

### `[p2p]`

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `secret_key_path` | `String` | No | Stable iroh endpoint key path. Mutually exclusive with `secret_key_seed_hex`. |
| `secret_key_seed_hex` | `String` | No | Deterministic test seed for loopback E2E. |
| `thin_node_url` | `String` | Yes | Coordination endpoint for registry and allow-list fetches. |
| `thin_node_token_file` | `String` | Yes | File containing the thin-node bearer token. |

### `[status_http]`

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `bind` | `String` | No | `127.0.0.1:8080` | Status HTTP bind address. |

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
token_file = "/etc/rusty-timer/forwarder-token.txt"

[journal]
sqlite_path = "/var/lib/rusty-timer/forwarder.sqlite3"
prune_watermark_pct = 80

[p2p]
secret_key_path = "/var/lib/rusty-timer/forwarder-endpoint.key"
thin_node_url = "https://thin-node.example.com"
thin_node_token_file = "/etc/rusty-timer/thin-node-token.txt"

[status_http]
bind = "127.0.0.1:8080"

[[readers]]
target = "192.168.1.50:10000"
enabled = true
```

## Operations

See [Forwarder operations](../../docs/runbooks/forwarder-operations.md) for
startup, recovery, retention, and epoch procedures.
