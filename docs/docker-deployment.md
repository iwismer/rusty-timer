# Docker Deployment Guide

This guide covers building and deploying the Remote Timing server stack using Docker and
Docker Compose. The forwarder runs as a systemd service on a Linux SBC and is covered
separately.

---

## Architecture Overview

```
 IPICO Reader(s)
       |
       | (TCP)
       v
 +---------------------+
 |  rt-forwarder       |  Linux SBC (Raspberry Pi or similar)
 |  (systemd service)  |  Config: /etc/rusty-timer/forwarder.toml
 +---------------------+
       |
       | (WebSocket, authenticated)
       v
 +---------------------+     +---------------------+
 |  rt-server          |<--->|  PostgreSQL 16       |
 |  (Docker container) |     |  (Docker container)  |
 +---------------------+     +---------------------+
       |
       | (HTTP :8080)
       +---> WebSocket ingest   (forwarders connect here)
       +---> Receiver delivery  (Windows receivers connect here)
       +---> REST API           (dashboard + receiver control)
       +---> Dashboard UI       (static SvelteKit, served by server)
       v
 Receiver (Windows, Tauri desktop app)
```

The server container bundles the SvelteKit dashboard as static files served directly by the
`rt-server` binary. No separate web server is required.

---

## Prerequisites

- **Docker** >= 24.0
- **Docker Compose** v2 (`docker compose`, not `docker-compose`)
- **PostgreSQL** is provided by the Compose stack; no separate installation is needed
- The repo root is required as the Docker build context for both Dockerfiles

---

## Environment Variables: Server

The server is configured exclusively via environment variables (no config file).

| Variable          | Required | Default                      | Description                                                    |
|-------------------|----------|------------------------------|----------------------------------------------------------------|
| `DATABASE_URL`    | Yes      | —                            | PostgreSQL connection string, e.g. `postgres://user:pass@host:5432/db` |
| `BIND_ADDR`       | No       | `0.0.0.0:8080`               | Address and port the server listens on                         |
| `LOG_LEVEL`       | No       | `info`                       | Log verbosity: `error`, `warn`, `info`, `debug`, `trace`       |
| `POSTGRES_PASSWORD` | Yes    | —                            | Set in `.env`; used by the Compose postgres service            |
| `POSTGRES_USER`   | No       | `rtserver`                   | Postgres username (Compose default)                            |
| `POSTGRES_DB`     | No       | `rtserver`                   | Postgres database name (Compose default)                       |
| `SERVER_PORT`     | No       | `8080`                       | Host port mapped to the server container                       |
| `SERVER_VERSION`  | No       | `latest`                     | Image tag used by the Compose server service                   |

In the Compose stack, `DATABASE_URL` is assembled automatically from `POSTGRES_USER`,
`POSTGRES_PASSWORD`, and `POSTGRES_DB`. You do not need to set it manually when using
`docker-compose.prod.yml`.

Create a `deploy/.env` file with at minimum:

```env
POSTGRES_PASSWORD=change-me-strong-password
```

---

## Forwarder Configuration

The forwarder uses **TOML configuration only** — it does not read environment variables for
its operational settings. The config file lives at `/etc/rusty-timer/forwarder.toml` on the
SBC.

See [docs/runbooks/forwarder-operations.md](runbooks/forwarder-operations.md) for full
configuration reference, installation steps, and operational procedures.

---

## Building Docker Images

Both Dockerfiles require the **repository root** as the build context because they copy
workspace-level files (`Cargo.toml`, `Cargo.lock`, `crates/`, etc.).

### Build the server image

```bash
docker build -t rt-server -f services/server/Dockerfile .
```

This runs a multi-stage build:
1. Builds the SvelteKit dashboard with Node 20.
2. Compiles the `server` binary with Rust (release mode).
3. Produces a minimal `debian:bookworm-slim` runtime image containing the binary and
   dashboard static files at `/srv/dashboard`.

The resulting image runs as the non-root `rt-server` user and exposes port 8080.

### Build the forwarder image (optional)

The forwarder is normally deployed as a native binary on the SBC, not as a container.
A Docker image is provided for CI or edge-case containerized deployments:

```bash
docker build -t rt-forwarder -f services/forwarder/Dockerfile .
```

The resulting image runs as the non-root `rt-forwarder` user and expects:
- `/etc/rusty-timer/` — TOML config (read-only mount)
- `/var/lib/rusty-timer/` — SQLite journal and token storage (read-write mount)

---

## Running with Docker Compose

The Compose file at `deploy/docker-compose.prod.yml` defines two services: `postgres` and
`server`. The forwarder is not included (it runs on the SBC via systemd).

### 1. Prepare the environment file

```bash
cp deploy/docker-compose.prod.yml deploy/docker-compose.prod.yml  # already present
# Create deploy/.env with at minimum:
echo "POSTGRES_PASSWORD=change-me-strong-password" > deploy/.env
```

### 2. Build the server image

```bash
docker build -t rt-server -f services/server/Dockerfile .
```

### 3. Start the stack

```bash
docker compose -f deploy/docker-compose.prod.yml up -d
```

This starts:
- `rt-postgres` (container name) — PostgreSQL 16, internal network only, data persisted in
  the `postgres_data` named volume
- `rt-server` (container name) — server binary, waits for Postgres to pass its health check
  before starting, port `8080` exposed to the host (or `SERVER_PORT` if overridden)

Postgres is not exposed externally. Use an SSH tunnel or a database admin tool connected
through the internal network for direct database access.

### 4. Verify the stack is up

```bash
docker compose -f deploy/docker-compose.prod.yml ps
```

---

## Health Checks

Both containers have built-in Docker health checks. You can also query the server directly:

```bash
curl http://localhost:8080/healthz
```

A healthy server returns HTTP 200. The Compose `server` service only starts after Postgres
reports healthy, so the server is always connected to the database at startup.

---

## Logs

View logs for all services:

```bash
docker compose -f deploy/docker-compose.prod.yml logs -f
```

View logs for a specific service:

```bash
# Server logs only
docker compose -f deploy/docker-compose.prod.yml logs -f server

# Postgres logs only
docker compose -f deploy/docker-compose.prod.yml logs -f postgres
```

---

## Stopping the Stack

Stop containers without removing volumes:

```bash
docker compose -f deploy/docker-compose.prod.yml down
```

Stop containers and remove the Postgres data volume (destructive — all timing data will be
lost):

```bash
docker compose -f deploy/docker-compose.prod.yml down -v
```

---

## Updating

To deploy a new server build:

```bash
# 1. Rebuild the image from the repository root
docker build -t rt-server -f services/server/Dockerfile .

# 2. Restart the server container with the new image
docker compose -f deploy/docker-compose.prod.yml up -d --no-deps server
```

Postgres data is stored in the `postgres_data` named volume and is not affected by
container restarts or image rebuilds.

---

## Token Registration

Forwarder and receiver authentication tokens are stored as SHA-256 hashes in the
`device_tokens` table in Postgres. Token registration is performed directly in the database.
See [docs/runbooks/server-operations.md](runbooks/server-operations.md) for the token
provisioning procedure.

---

## Forwarder Deployment (systemd on SBC)

The forwarder is a native Rust binary deployed on the Linux SBC, not a container. Key
points:

- **Cross-compilation target:** The SBC (e.g., Raspberry Pi) typically requires
  `aarch64-unknown-linux-gnu` or `armv7-unknown-linux-gnueabihf`. Build with
  `cargo build --release --package forwarder --bin forwarder --target <target>`, then copy
  the binary to `/usr/local/bin/rt-forwarder` on the SBC.
- **Systemd unit:** `deploy/systemd/rt-forwarder.service` — copy to
  `/etc/systemd/system/rt-forwarder.service` and enable with `systemctl enable --now rt-forwarder`.
- **Config:** `/etc/rusty-timer/forwarder.toml` (TOML only, no environment variable
  overrides for operational settings).
- **Journal storage:** `/var/lib/rusty-timer/` (SQLite WAL, power-loss-safe).

Full installation and configuration details are in
[docs/runbooks/forwarder-operations.md](runbooks/forwarder-operations.md).
