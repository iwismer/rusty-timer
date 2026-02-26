# Local Manual Testing Guide

This guide walks through starting every component of the Remote Forwarding Suite on a
single development machine and exercising the end-to-end flow manually.

---

## Prerequisites

| Tool | Required version | Notes |
|------|-----------------|-------|
| Rust | 1.89.0 | Managed by `rust-toolchain.toml`; `rustup` picks it up automatically |
| Docker | any recent | Used to run Postgres |
| Node.js + npm | 24.x + 11.x | Required for frontend apps; use `nvm use` from repo root |
| jq | optional | Used by the pre-commit hook for JSON formatting checks |

All Rust binaries are built from the workspace root
`/Users/iwismer/Documents/rusty-timer` (or wherever you cloned the repo).

---

## Step 1: Start Postgres

The server requires a running Postgres instance. The credentials below match the
default `DATABASE_URL` used in the examples that follow.

```bash
docker run --rm -d \
  --name rt-postgres \
  -e POSTGRES_USER=rt \
  -e POSTGRES_PASSWORD=secret \
  -e POSTGRES_DB=rusty_timer \
  -p 5432:5432 \
  postgres:16
```

Wait a few seconds for the container to finish initialising before starting the server.

To stop it later:

```bash
docker stop rt-postgres
```

---

## Step 2: Run the Server

The server reads configuration exclusively from environment variables. Migrations are
applied automatically on startup.

**Required env var:**

| Variable | Description |
|----------|-------------|
| `DATABASE_URL` | Postgres connection string |

**Optional env vars (with defaults):**

| Variable | Default | Description |
|----------|---------|-------------|
| `BIND_ADDR` | `0.0.0.0:8080` | TCP address the HTTP/WS server listens on |
| `LOG_LEVEL` | `info` | Tracing filter (e.g. `debug`, `info`, `warn`) |

```bash
DATABASE_URL=postgres://rt:secret@localhost:5432/rusty_timer \
BIND_ADDR=0.0.0.0:8080 \
cargo run --release -p server
```

The server exposes:

- `GET  /healthz` and `GET /readyz` — liveness/readiness probes
- `WS   /ws/v1/forwarders` — forwarder WebSocket ingest
- `WS   /ws/v1.2/receivers` — receiver WebSocket fanout
- `GET  /api/v1/streams` — list all known streams
- `PATCH /api/v1/streams/:stream_id` — rename a stream (`display_alias`)
- `GET  /api/v1/streams/:stream_id/metrics` — per-stream counters
- `GET  /api/v1/streams/:stream_id/export.txt` — raw read export
- `GET  /api/v1/streams/:stream_id/export.csv` — CSV read export
- `POST /api/v1/streams/:stream_id/reset-epoch` — increment the stream epoch
- `GET  /api/v1/announcer/config` — read announcer config
- `PUT  /api/v1/announcer/config` — update announcer config
- `POST /api/v1/announcer/reset` — reset announcer runtime state
- `GET  /api/v1/announcer/state` — full announcer snapshot (internal/operator)
- `GET  /api/v1/announcer/events` — full announcer SSE updates (internal/operator)
- `GET  /api/v1/public/announcer/state` — sanitized public announcer snapshot
- `GET  /api/v1/public/announcer/events` — sanitized public announcer SSE updates

---

## Step 3: Configure and Run the Forwarder

The forwarder reads its configuration exclusively from a TOML file. The default path
is `/etc/rusty-timer/forwarder.toml`. There is currently no command-line flag to
override the path; place the file at the default location or create a symlink.

### Token file

The forwarder authenticates to the server with a bearer token. Create a plain-text
file containing the raw token on a single line:

```bash
echo "my-dev-token" > /tmp/forwarder-token.txt
```

The server looks up `SHA-256(raw_token_bytes)` in the `device_tokens` table. For
development you need to insert a matching row (see the schema in
`services/server/migrations/0001_init.sql`).

### Example TOML config

```toml
schema_version = 1

[server]
base_url            = "ws://127.0.0.1:8080"
# forwarders_ws_path defaults to "/ws/v1/forwarders"

[auth]
token_file = "/tmp/forwarder-token.txt"

[journal]
sqlite_path         = "/tmp/forwarder-journal.sqlite3"
prune_watermark_pct = 80

[status_http]
bind = "0.0.0.0:8081"

[uplink]
batch_mode       = "immediate"
batch_flush_ms   = 100
batch_max_events = 50

[[readers]]
target               = "127.0.0.1:10001"
read_type            = "raw"
enabled              = true
# local_fallback_port is optional; defaults to 10000 + last octet of reader IP
```

Save this file to `/etc/rusty-timer/forwarder.toml` (create the directory first if
needed):

```bash
sudo mkdir -p /etc/rusty-timer
sudo cp forwarder.toml /etc/rusty-timer/forwarder.toml
```

### Run

```bash
cargo run --release -p forwarder
```

The forwarder also exposes a status HTTP endpoint at the `status_http.bind` address
(default `0.0.0.0:8081`).

**Required TOML fields:**

| Field | Description |
|-------|-------------|
| `schema_version` | Must be `1` |
| `server.base_url` | WebSocket URL of the server |
| `auth.token_file` | Path to the plain-text bearer token file |
| `[[readers]]` | At least one reader entry; each needs `target` |

---

## Step 4: Run the Emulator (original TCP emulator)

The original emulator (`services/emulator`) listens on a TCP port and streams
synthetic IPICO-format chip reads to whatever connects to it. The forwarder's reader
`target` should point at the emulator's address.

**CLI flags:**

| Flag | Short | Default | Description |
|------|-------|---------|-------------|
| `--port` | `-p` | `10001` | TCP port to listen on |
| `--delay` | `-d` | `1000` | Milliseconds between reads |
| `--type` | `-t` | `raw` | Read type (`raw` or others) |
| `--file` | `-f` | none | Optional file of pre-recorded reads |

```bash
cargo run --release -p emulator -- --port 10001 --delay 2000 --type raw
```

This matches the `target = "127.0.0.1:10001"` entry in the example config above. The
forwarder connects to the emulator; start the emulator before the forwarder (or let
the forwarder retry on reconnect).

---

## Step 5: Run the Dashboard

The dashboard is a SvelteKit static app that talks to the server's HTTP API.

```bash
cd apps/server-ui
npm install
npm run dev
```

The dev server opens at **http://localhost:5173**. It proxies API calls to the server
running at `http://localhost:8080` (configure `vite.config.*` if your server is on a
different port).

Other useful npm scripts:

| Script | Purpose |
|--------|---------|
| `npm run build` | Production build |
| `npm run preview` | Serve the production build locally |
| `npm test` | Run Vitest unit tests (no browser required) |
| `npm run check` | TypeScript and Svelte type-checking |

### Announcer local smoke

With server + dashboard running:

1. Open `http://localhost:5173/announcer-config`
2. Enable announcer and select at least one stream
3. Open `http://localhost:5173/announcer` in another tab/window
4. Generate reads from emulator/forwarder; verify newest-first list updates live
5. Use reset button and verify list/count clear immediately

Notes:
- `enabled_until` is fixed when you explicitly enable announcer.
- Config edits do not extend expiry.
- No historical backfill appears after enable/reset; only new reads.

---

## Step 6: Run the Receiver

The receiver is a Windows-targeted service that subscribes to streams from the server
and re-exposes them as local TCP listeners. It can be compiled and run on any platform
for development.

```bash
cargo run --release -p receiver
```

The receiver stores its SQLite profile in the platform's local data directory
(e.g. `~/.local/share/rusty-timer/receiver/receiver.sqlite3` on Linux,
`%LOCALAPPDATA%\rusty-timer\receiver\receiver.sqlite3` on Windows). The directory is
created automatically.

The control API listens on **127.0.0.1:9090**.

---

## Step 7: Subscribe to a Stream via the Control API

All control API calls go to `http://127.0.0.1:9090`.

> **Note:** When using `uv run scripts/dev.py` (tmux or iTerm2 launch), the
> receiver profile is configured automatically with the dev token and server URL,
> and a connect request is sent. You can skip the profile and connect steps
> below and go straight to subscribing. To re-run the configuration manually:
> `/tmp/rusty-timer-dev/configure-receiver.sh`

### Set the server profile (credentials)

```bash
curl -s -X PUT http://127.0.0.1:9090/api/v1/profile \
  -H 'Content-Type: application/json' \
  -d '{"server_url":"ws://127.0.0.1:8080","token":"rusty-dev-receiver","log_level":"info"}'
```

Returns `204 No Content` on success.

The `server_url` is the base URL of the server (just the scheme, host, and port). The
receiver automatically appends `/ws/v1.2/receivers` when connecting, so the full WebSocket
URL becomes `ws://127.0.0.1:8080/ws/v1.2/receivers`.

The default dev token for the receiver is `rusty-dev-receiver`.

### Connect to the server

```bash
curl -s -X POST http://127.0.0.1:9090/api/v1/connect
```

Returns `202 Accepted` (connection attempt is asynchronous).

### Check connection status

```bash
curl -s http://127.0.0.1:9090/api/v1/status | jq .
```

Example response:

```json
{
  "connection_state": "connected",
  "local_ok": true,
  "streams_count": 0
}
```

### Subscribe to a stream

Replace `forwarder-001` and `192.168.1.1` with actual values visible in
`GET /api/v1/streams` on the server.

```bash
curl -s -X PUT http://127.0.0.1:9090/api/v1/subscriptions \
  -H 'Content-Type: application/json' \
  -d '{
    "subscriptions": [
      {
        "forwarder_id": "forwarder-001",
        "reader_ip": "192.168.1.1"
      }
    ]
  }' | jq .
```

Returns `204 No Content`. The receiver opens a local TCP listener on port
`10000 + last_octet(reader_ip)` (e.g. port `10001` for `192.168.1.1`) and forwards
incoming events to any client connected to that port.

Use `local_port_override` to choose a specific port:

```json
{
  "subscriptions": [
    {
      "forwarder_id": "forwarder-001",
      "reader_ip": "192.168.1.1",
      "local_port_override": 9500
    }
  ]
}
```

### List streams (merged server + local subscriptions)

```bash
curl -s http://127.0.0.1:9090/api/v1/streams | jq .
```

### Disconnect

```bash
curl -s -X POST http://127.0.0.1:9090/api/v1/disconnect
```

### Full control API reference

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/api/v1/profile` | Read current profile |
| `PUT` | `/api/v1/profile` | Save server URL, token, log level |
| `GET` | `/api/v1/streams` | List streams (server + local subs) |
| `PUT` | `/api/v1/subscriptions` | Replace subscription list |
| `GET` | `/api/v1/status` | Runtime connection state |
| `GET` | `/api/v1/logs` | Recent log entries |
| `POST` | `/api/v1/connect` | Initiate WS connection (async, 202) |
| `POST` | `/api/v1/disconnect` | Close WS connection (async, 202) |

---

## End-to-End Flow Summary

```
 emulator (TCP :10001)
      |
      |  TCP (IPICO format)
      v
 forwarder  ──── WS /ws/v1/forwarders ────>  server (:8080)
                                                  |
                                    +─────────────+──────────────+
                                    |                            |
                              dashboard (HTTP API)        receiver ──> local TCP
                              (:5173 dev)                 (:9090 ctrl)  (:10000+N)
```

Flow description:

1. The **emulator** listens on TCP and streams synthetic chip reads.
2. The **forwarder** connects to the emulator TCP socket, parses the reads, journals
   them to SQLite, and forwards them upstream over a WebSocket to the server.
3. The **server** ingests events, persists them to Postgres, and fans them out to
   connected receivers over a second WebSocket endpoint.
4. The **dashboard** queries the server HTTP API to display stream lists, metrics, and
   export data.
5. The **receiver** subscribes to specific streams, receives batches from the server,
   acknowledges them with cursor updates, and re-exposes the raw bytes on local TCP
   ports for consumption by local timing software.

---

## Running All Automated Tests

### Unit and integration tests (no Docker required)

```bash
# Run all unit tests across every crate in the workspace
cargo test --workspace
```

### Full test suite including integration tests (Docker required)

Integration tests spin up Postgres via Testcontainers. Use `--test-threads` to avoid
resource contention:

```bash
cargo test --workspace -- --test-threads=4
```

### Dashboard unit tests

```bash
cd apps/server-ui
npm test
```

### Dashboard end-to-end tests (Playwright)

```bash
cd apps/server-ui
npx playwright install --with-deps
npm run test:e2e
```

### Packaging validation

```bash
bash scripts/validate-packaging.sh
```

This script runs approximately 43 bash checks that verify binary presence, config
file paths, and packaging artefacts.
