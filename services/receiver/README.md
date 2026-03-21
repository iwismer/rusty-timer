# Receiver

The receiver is a library crate that subscribes to live timing streams from a
rusty-timer server over WebSocket, re-exposes each stream as a local TCP port
for race-management software, and provides the business logic for configuration
and monitoring.

It is embedded in the Tauri desktop app (`apps/receiver-ui/src-tauri`). All
configuration is stored in a local SQLite profile and managed through the
Tauri UI via IPC commands.

## Build

The receiver is built as a library dependency of the Tauri app:

```bash
cd apps/receiver-ui && cargo tauri build
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

## API

The receiver exposes its functionality as plain async functions in `control_api.rs`.
These are called by the Tauri app via IPC commands. There is no standalone HTTP API.

## Subscription model

Subscriptions are managed via `GET /api/v1/subscriptions` and
`PUT /api/v1/subscriptions`. The PUT request body replaces the entire
subscription list atomically:

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

## Receiver mode behavior (v1.2)

- `live`: subscribes to explicit streams and can apply earliest-epoch overrides.
- `race`: subscribes to streams linked to the selected race ID.
- `targeted_replay`: replays explicit `(forwarder_id, reader_ip, stream_epoch, from_seq)` targets.

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

## Troubleshooting

### `PUT /api/v1/mode` with race mode returns 400

If you call `PUT /api/v1/mode` with `{"mode":"race","race_id":""}` you will get
`400 Bad Request` because `race_id` must be non-empty.

**Fix:** Supply a non-empty race ID in race mode payloads.
