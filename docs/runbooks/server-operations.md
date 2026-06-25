# Server Operations Runbook

This runbook covers startup, recovery, and operator checks for the `server`
service in the P2P architecture. The server stores device registry state,
distributes the receiver allow-list to forwarders, and hosts the announcer row
status board.

## Service Overview

The server is a small Axum service backed by SQLite. It provides:

- `GET /healthz` and `GET /status` public read endpoints.
- Device bootstrap/recovery registration via `POST /register`.
- Forwarder stream catalog updates via `POST /forwarder/catalog`.
- Receiver discovery via `GET /forwarders`.
- Receiver allow-list distribution via `GET /allowlist/receivers`.
- Fenced announcer push via `POST /announcer/takeover` and `POST /announcer/rows`.
- Admin approval via `POST /admin/devices/approve`, expected to sit behind
  Caddy + Authelia.
- Admin enrollment-token management via `/admin/enrollment-tokens`.

The service does not use `EndpointId` over plain HTTP as an authenticator.
Device routes use server-minted per-device bearer tokens. New or recovering
devices first present an admin-issued enrollment voucher to `/register`; the
server consumes the voucher and returns the minted token once. Admin identity is
delegated to the reverse proxy.

## Startup and Installation

For Docker deployments, see `deploy/server/`. The sample Compose deployment
runs the server with an embedded UI behind Caddy and persists SQLite state in a
Docker volume.

For a binary install on a Linux server host:

```bash
sudo install -m 0755 server /usr/local/bin/server
sudo useradd -r -s /bin/false -m -d /var/lib/rusty-timer-server rt-server || true
sudo install -d -o rt-server -g rt-server -m 0750 /var/lib/rusty-timer-server
```

Set environment variables in the systemd unit or environment file:

```bash
BIND_ADDR=127.0.0.1:8080
SERVER_DB_PATH=/var/lib/rusty-timer-server/server.sqlite3
LOG_LEVEL=info
# Set only when a trusted header-stripping proxy injects Remote-User.
SERVER_TRUSTED_PROXY=1
```

Start manually for a smoke test:

```bash
sudo -u rt-server \
  BIND_ADDR=127.0.0.1:8080 \
  SERVER_DB_PATH=/var/lib/rusty-timer-server/server.sqlite3 \
  /usr/local/bin/server
```

Verify startup:

```bash
curl -fsS http://127.0.0.1:8080/healthz
curl -fsS http://127.0.0.1:8080/status
```

## Auth Posture

Use Caddy + Authelia in front of the service for public deployments:

| Route | Auth posture |
| --- | --- |
| `GET /healthz`, `GET /status` | Public read; no secrets returned. |
| `/admin/*` | Admin only; Authelia must inject `Remote-User`. |
| `POST /register` | Enrollment voucher or the device's own minted bearer token. |
| `POST /forwarder/catalog` | Matching forwarder minted bearer token; approval is not required so pending forwarders can publish catalog data for approval. |
| `GET /allowlist/receivers` | Active forwarder minted bearer token. |
| `GET /forwarders` | Active receiver minted bearer token. |
| `POST /announcer/takeover`, `POST /announcer/rows` | Active receiver minted bearer token. |

Caddy must strip any inbound client-supplied `Remote-User` header before proxying
to the server. Only the trusted proxy may set that header after Authelia admin
authentication. The server denies `/admin/*` unless `SERVER_TRUSTED_PROXY=1` is
set.

## Provisioning and Device Approval

Forwarders may be provisioned from the Server UI `SBC Setup` tab. Generate or
add a forwarder enrollment token, copy the one-time secret into the setup form,
download `user-data` and `network-config`, then boot the SBC. Generated token
secrets are shown only once; the token list exposes metadata only.

Receivers also need an enrollment voucher before first registration. Create a
receiver token from the Server UI `Admin` tab: under **Receiver enrollment
tokens**, generate (or add a manual) token and copy the one-time secret. Enter
that secret as the token, along with the server URL and receiver ID, in the
receiver app's `Config` tab to register the receiver as `pending`. As with
forwarder tokens, generated receiver secrets are shown only once and the token
list exposes metadata only.

After a device presents its voucher to `/register`, the server mints a
per-device token and stores only its hash. Devices should persist the minted
token and use it for steady-state server calls.

New devices start as `pending`. An admin approves a device through the protected
admin route:

```bash
curl -fsS -X POST http://127.0.0.1:8080/admin/devices/approve \
  -H 'Remote-User: admin@example.com' \
  -H 'Content-Type: application/json' \
  -d '{"endpoint_id":"receiver-finish-line"}'
```

## Enrollment Token Revocation

Revoking an unused enrollment token prevents first registration. Revoking a used
token blocks recovery/re-registration with that voucher; devices that already
persisted a minted per-device token continue to authenticate with that minted
token unless the device itself is deactivated or re-enrolled.

## Allow-list Distribution

Forwarders fetch the active receiver allow-list with their minted forwarder
bearer token:

```bash
curl -fsS http://127.0.0.1:8080/allowlist/receivers \
  -H "Authorization: Bearer ${FORWARDER_DEVICE_TOKEN}"
```

Only active receivers appear in the response. Pending devices and forwarders are
excluded. If a receiver is revoked or no longer active, forwarders should stop
allowing new connections for that endpoint and close any open revoked connection
as their local allow-list refreshes.

## Announcer Push and Fenced Generation

Receivers take over a fenced announcer source generation before pushing rows:

```bash
curl -fsS -X POST http://127.0.0.1:8080/announcer/takeover \
  -H "Authorization: Bearer ${RECEIVER_DEVICE_TOKEN}"
```

Each `POST /announcer/rows` request includes the returned
`announcer_source_generation`, `stream_id`, and monotonic `seq`. The server
rejects stale generations with `409 Conflict`, which prevents an older receiver
process from overwriting a newer source after restart or failover.

## Recovery Procedures

### Service restart

The server stores registry, allow-list, and announcer state in SQLite. On
restart, point `SERVER_DB_PATH` at the same file and start the service again:

```bash
sudo systemctl restart rt-server
curl -fsS http://127.0.0.1:8080/healthz
```

Forwarders and receivers retry registration, allow-list fetches, discovery, and
announcer pushes. No manual cursor migration is required.

### Database backup and restore

Stop the service before copying the SQLite file:

```bash
sudo systemctl stop rt-server
sudo cp /var/lib/rusty-timer-server/server.sqlite3 /tmp/server.sqlite3.bak
sudo systemctl start rt-server
```

To restore, stop the service, replace the SQLite file with the backup, then
start the service. Devices may need a fresh enrollment voucher if the backup
predates their minted device token.

### Lost device token

Issue a new enrollment voucher from the protected admin UI/API and reconfigure
the affected device with that voucher. The next `/register` call mints a fresh
per-device token for the endpoint.

## Troubleshooting

- `401 Unauthorized` on `/register`: check whether the bearer is an unused
  enrollment voucher of the requested device kind or the device's own minted
  token for the same endpoint.
- `401 Unauthorized` on `/forwarder/catalog` or `/allowlist/receivers`: confirm
  the request uses the matching forwarder's minted token.
- `401 Unauthorized` on `/forwarders` or `/announcer/*`: confirm the request
  uses an active receiver's minted token.
- Empty allow-list: confirm the receiver registered and an admin approved it as
  active; pending receivers are intentionally excluded.
- `409 Conflict` on announcer row push: the receiver has a stale generation;
  retry takeover and resend durable rows with the newer generation.
- Admin approval does not work: verify Caddy + Authelia injected `Remote-User`,
  stripped any untrusted client-supplied copy, and the server has
  `SERVER_TRUSTED_PROXY=1`.
