# Server

The server is the SQLite-backed coordination service for the P2P architecture.
It stores device registrations, receiver allow-list state, forwarder stream
catalogs, and announcer/status-board rows. It does not carry raw chip-read data;
forwarders and receivers exchange reads directly over iroh.

## Build

```bash
cargo build --release -p server
```

To include the embedded server UI:

```bash
cargo build --release -p server --features embed-ui
```

## Configuration

The server is configured with environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `BIND_ADDR` | `0.0.0.0:8080` | HTTP bind address. |
| `SERVER_DB_PATH` | `server.sqlite3` | SQLite registry/status database path. |
| `LOG_LEVEL` | `info` | Tracing filter passed to `tracing_subscriber`. |
| `SERVER_TRUSTED_PROXY` | unset/false | Enables `/admin/*` routes to trust a header-stripping reverse proxy's `Remote-User` header. Accepted true values are `1`, `true`, `yes`, and `on`. |

`/admin/*` routes fail closed unless `SERVER_TRUSTED_PROXY` is enabled. Only set
it when the service is reachable exclusively through a trusted proxy that strips
client-supplied `Remote-*` headers before injecting `Remote-User`.

## Routes

| Route | Auth | Purpose |
|-------|------|---------|
| `GET /healthz` | Public | Process health check. |
| `GET /status` | Public | Status board with device, stream, and announcer state. |
| `POST /register` | Enrollment voucher or device token | Bootstrap/recover a forwarder or receiver and mint a per-device token. |
| `POST /forwarder/catalog` | Matching forwarder token | Publish a forwarder's stream catalog. |
| `GET /allowlist/receivers` | Active forwarder token | Fetch approved receiver endpoint IDs for forwarder allow-list enforcement. |
| `GET /forwarders` | Active receiver token | Discover approved forwarders and streams. |
| `POST /announcer/takeover` | Active receiver token | Take over the fenced announcer generation. |
| `POST /announcer/rows` | Active receiver token | Push sanitized announcer rows. |
| `POST /admin/devices/approve` | Trusted `Remote-User` | Approve a pending device. |
| `/admin/enrollment-tokens` | Trusted `Remote-User` | Create/list/revoke enrollment vouchers. |

## Local smoke test

```bash
BIND_ADDR=127.0.0.1:8080 \
SERVER_DB_PATH=/tmp/rusty-timer-server.sqlite3 \
cargo run -p server
```

Then verify:

```bash
curl -fsS http://127.0.0.1:8080/healthz
curl -fsS http://127.0.0.1:8080/status
```

## Operations

See [Server operations](../../docs/runbooks/server-operations.md) and
[Docker deployment](../../deploy/server/README.md).
