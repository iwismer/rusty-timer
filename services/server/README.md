# rt-server

Central hub for the Rusty Timer system. Built with Axum and PostgreSQL, the
server ingests timing reads from forwarders, deduplicates them, fans out live
data over WebSocket/SSE, and exposes a REST API plus an optional web dashboard.

PostgreSQL migrations run automatically on startup.

## Build

```bash
cargo build --release -p server
```

The binary is written to `target/release/server`.

## Configuration

The server is configured entirely through environment variables (no config file
or CLI flags).

| Variable | Required | Default | Description |
|---|---|---|---|
| `DATABASE_URL` | Yes | -- | PostgreSQL connection string (e.g. `postgres://user:pass@localhost:5432/rtserver`) |
| `BIND_ADDR` | No | `0.0.0.0:8080` | Address and port the HTTP server listens on |
| `DASHBOARD_DIR` | No | -- | Path to the built dashboard static files. When set, the server serves the web UI as a fallback for non-API routes. |
| `LOG_LEVEL` | No | `info` | Tracing filter directive (e.g. `debug`, `warn`, `server=debug,sqlx=warn`) |

## Usage

```bash
export DATABASE_URL="postgres://user:pass@localhost:5432/rtserver"
export BIND_ADDR="0.0.0.0:8080"
export LOG_LEVEL="info"

cargo run --release -p server
```

## API overview

All REST endpoints live under `/api/v1/`. WebSocket endpoints are under `/ws/v1/`.

### Health

- `GET /healthz` -- liveness probe
- `GET /readyz` -- readiness probe

### Streams

- `GET /api/v1/streams` -- list all timing streams
- `PATCH /api/v1/streams/:stream_id` -- update stream metadata
- `GET /api/v1/streams/:stream_id/metrics` -- stream metrics
- `GET /api/v1/streams/:stream_id/export.{txt,csv}` -- export reads
- `POST /api/v1/streams/:stream_id/reset-epoch` -- reset the stream epoch
- `GET /api/v1/streams/:stream_id/epochs` -- list epochs

### Reads

- `GET /api/v1/streams/:stream_id/reads` -- reads for a stream
- `GET /api/v1/forwarders/:forwarder_id/reads` -- reads for a forwarder

### Announcer

- `GET /api/v1/announcer/config` -- read announcer config
- `PUT /api/v1/announcer/config` -- update announcer config
- `POST /api/v1/announcer/reset` -- clear announcer runtime state
- `GET /api/v1/announcer/state` -- public snapshot (config + current rows/count)
- `GET /api/v1/announcer/events` -- announcer SSE updates (`announcer_update`)

Announcer semantics:
- Enabling requires at least one selected stream.
- `enabled_until` is set on explicit enable and is not extended by later edits.
- Runtime state (dedup/list/count) is in-memory and resets on process restart.
- No historical backfill is emitted on enable/reset; only new canonical reads.

### Races

- `GET /api/v1/races` -- list races
- `POST /api/v1/races` -- create a race
- `DELETE /api/v1/races/:race_id` -- delete a race
- `GET /api/v1/races/:race_id/participants` -- list participants
- `POST /api/v1/races/:race_id/participants/upload` -- upload participants
- `POST /api/v1/races/:race_id/chips/upload` -- upload chip assignments
- `GET /api/v1/forwarder-races` -- list forwarder-race associations
- `GET|PUT /api/v1/forwarders/:forwarder_id/race` -- get/set the active race for a forwarder

### Forwarder config

- `GET /api/v1/forwarders/:forwarder_id/config` -- read forwarder config
- `POST /api/v1/forwarders/:forwarder_id/config/:section` -- update a config section
- `POST /api/v1/forwarders/:forwarder_id/restart` -- request forwarder restart

### Admin

Token management, bulk deletion of streams/events, and receiver-cursor
management. See `src/http/admin.rs` for the full list.

### WebSocket

- `GET /ws/v1/forwarders` -- forwarder uplink (timing data ingest)
- `GET /ws/v1.2/receivers` -- receiver downlink (mode-based fan-out)

### Server-Sent Events

- `GET /api/v1/events` -- SSE stream for the dashboard
- `GET /api/v1/announcer/events` -- SSE stream for announcer rows

## Docker

The Dockerfile is a multi-stage build that compiles both the SvelteKit dashboard
and the Rust server binary. It must be built from the repository root.

```bash
docker build -t rt-server -f services/server/Dockerfile .
```

Run the image:

```bash
docker run --rm \
  -e DATABASE_URL="postgres://user:pass@db:5432/rtserver" \
  -e BIND_ADDR="0.0.0.0:8080" \
  -e DASHBOARD_DIR="/srv/dashboard" \
  -p 8080:8080 \
  rt-server
```

The container runs as a non-root user and includes a health check against
`/healthz`.

## Deployment

For a production Docker Compose setup (server + PostgreSQL), see
[`deploy/server/`](../../deploy/server/).
