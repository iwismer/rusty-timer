# Receiver

The receiver subscribes to live timing streams from a rusty-timer server over
WebSocket, re-exposes each stream as a local TCP port for race-management
software, and provides an embedded web UI for configuration and monitoring.

It is designed to run on end-user machines (laptops, desktops) and requires no
CLI arguments -- all configuration is stored in a local SQLite profile and
managed through the control API or the web UI.

## Build

```bash
cargo build --release -p receiver
```

To include the embedded web UI in the binary:

```bash
cargo build --release -p receiver --features embed-ui
```

## Data storage

The receiver persists its profile, subscriptions, and stream cursors in a SQLite
database at a platform-specific data directory:

| Platform | Path |
|----------|------|
| Linux    | `~/.local/share/rusty-timer/receiver/receiver.sqlite3` |
| macOS    | `~/Library/Application Support/rusty-timer/receiver/receiver.sqlite3` |
| Windows  | `%LOCALAPPDATA%\rusty-timer\receiver\receiver.sqlite3` |

The directory is created automatically on first run.

## Control API

The control API binds to `127.0.0.1:9090` and is not configurable. All
endpoints are JSON unless otherwise noted.

| Method | Path | Description |
|--------|------|-------------|
| `GET`  | `/api/v1/profile` | Read the current profile (server URL, token, log level). Returns `404` if no profile is configured. |
| `PUT`  | `/api/v1/profile` | Create or replace the profile. |
| `GET`  | `/api/v1/streams` | List all known streams, merging upstream server data with local subscriptions. |
| `PUT`  | `/api/v1/subscriptions` | Replace the full subscription list (atomic). |
| `GET`  | `/api/v1/status` | Runtime status: connection state, stream count, DB health. |
| `GET`  | `/api/v1/logs` | Recent log entries (up to 500, in-memory ring buffer). |
| `POST` | `/api/v1/connect` | Initiate a WebSocket connection to the server. Returns `202 Accepted`. |
| `POST` | `/api/v1/disconnect` | Close the active WebSocket connection. Returns `202 Accepted`. |
| `GET`  | `/api/v1/events` | SSE stream of real-time UI events (`status_changed`, `streams_snapshot`, `log_entry`, `update_available`). |
| `GET`  | `/api/v1/update/status` | Check for available software updates. |
| `POST` | `/api/v1/update/apply` | Apply a previously downloaded update. Returns `404` if no update is staged. |

Any path not matching an API route is served by the embedded web UI (when built
with `--features embed-ui`).

### `PUT /api/v1/profile`

```json
{
  "server_url": "ws://timing.example.com:8080",
  "token": "your-auth-token",
  "log_level": "info"
}
```

The `server_url` is normalized on save: a `ws://` scheme is prepended if no
scheme is provided, and trailing slashes are stripped.

### `GET /api/v1/status`

```json
{
  "connection_state": "connected",
  "local_ok": true,
  "streams_count": 3
}
```

`connection_state` is one of `disconnected`, `connecting`, `connected`, or
`disconnecting`.

### `GET /api/v1/streams`

```json
{
  "streams": [
    {
      "forwarder_id": "fwd-001",
      "reader_ip": "192.168.1.100:10000",
      "subscribed": true,
      "local_port": 10100,
      "online": true,
      "display_alias": "Start Line"
    }
  ],
  "degraded": false,
  "upstream_error": null
}
```

The response merges locally subscribed streams with upstream availability data.
When the receiver is not connected to the server, `degraded` is `true` and
`upstream_error` explains why.

## Subscription model

Subscriptions are managed via `PUT /api/v1/subscriptions`. The request body
replaces the entire subscription list atomically:

```json
{
  "subscriptions": [
    {
      "forwarder_id": "fwd-001",
      "reader_ip": "192.168.1.100:10000",
      "local_port_override": null
    },
    {
      "forwarder_id": "fwd-001",
      "reader_ip": "192.168.1.200:10000",
      "local_port_override": 9500
    }
  ]
}
```

Each subscription identifies a stream by its `forwarder_id` and `reader_ip`.
The optional `local_port_override` lets you choose a specific local TCP port
for that stream (see Port assignment below).

Duplicate `(forwarder_id, reader_ip)` pairs in the same request are rejected
with `400 Bad Request`.

## Port assignment

Each subscribed stream is re-exposed as a TCP listener on localhost. The local
port is determined as follows:

1. If `local_port_override` is set on the subscription, that port is used.
2. Otherwise the default is `10000 + last_octet(reader_ip)`.
   - For example, a reader at `192.168.1.100` maps to local port `10100`.
   - Readers using the legacy source port (`:10000`) or no port suffix follow
     this rule.
3. Readers with a non-default source port (anything other than `:10000`) are
   mapped to a deterministic hashed port in the range `12000-65535`.

If two subscriptions resolve to the same port (collision), both are skipped and
logged as degraded. Non-colliding streams start normally.

## Logging

The receiver uses the `RUST_LOG` environment variable to control log verbosity
(via `tracing-subscriber`). The default level is `info`.

```bash
RUST_LOG=debug receiver
```
