# Thin-node Operations Runbook

This runbook covers startup, recovery, and operator checks for the `thin-node`
service in the P2P architecture. The thin node stores device registry state,
distributes the receiver allow-list to forwarders, and hosts the announcer row
status board.

## Service Overview

The thin node is a small Axum service backed by SQLite. It provides:

- `GET /healthz` and `GET /status` public read endpoints.
- TOFU device registration via `POST /register`.
- Receiver allow-list distribution via `GET /allowlist/receivers`.
- Fenced announcer push via `POST /announcer/takeover` and `POST /announcer/rows`.
- Admin approval via `POST /admin/devices/approve`, expected to sit behind
  Caddy + Authelia.

The service does not use `EndpointId` over plain HTTP as an authenticator. Device
routes use a provisioning bearer token; admin identity is delegated to the
reverse proxy.

## Startup and Installation

Install an arm64 release artifact on the thin-node host:

```bash
sudo install -m 0755 thin-node /usr/local/bin/thin-node
sudo useradd -r -s /bin/false -m -d /var/lib/rusty-timer-thin-node rt-thin-node || true
sudo install -d -o rt-thin-node -g rt-thin-node -m 0750 /var/lib/rusty-timer-thin-node
```

Set the required environment variables in the systemd unit or environment file:

```bash
BIND_ADDR=127.0.0.1:8080
THIN_NODE_DB_PATH=/var/lib/rusty-timer-thin-node/thin-node.sqlite3
THIN_NODE_PROVISIONING_TOKEN=<long random provisioning token>
LOG_LEVEL=info
```

Start manually for a smoke test:

```bash
sudo -u rt-thin-node \
  BIND_ADDR=127.0.0.1:8080 \
  THIN_NODE_DB_PATH=/var/lib/rusty-timer-thin-node/thin-node.sqlite3 \
  THIN_NODE_PROVISIONING_TOKEN="$THIN_NODE_PROVISIONING_TOKEN" \
  /usr/local/bin/thin-node
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
| `POST /admin/devices/approve` | Admin only; Authelia must inject `Remote-User`. |
| `POST /register` | M2M/device `Authorization: Bearer <THIN_NODE_PROVISIONING_TOKEN>`. |
| `GET /allowlist/receivers` | M2M/device bearer token. |
| `POST /announcer/takeover`, `POST /announcer/rows` | M2M/device bearer token. |

Caddy must strip any inbound client-supplied `Remote-User` header before proxying
to the thin node. Only the trusted proxy may set that header after Authelia
admin authentication.

## Provisioning and Device Approval

Forwarders and receivers self-register with the provisioning token:

```bash
curl -fsS -X POST http://127.0.0.1:8080/register \
  -H "Authorization: Bearer ${THIN_NODE_PROVISIONING_TOKEN}" \
  -H 'Content-Type: application/json' \
  -d '{
    "endpoint_id": "receiver-finish-line",
    "device_kind": "receiver",
    "device_token": "per-device-random-token"
  }'
```

New devices start as `pending`. An admin approves a device through the protected
admin route:

```bash
curl -fsS -X POST http://127.0.0.1:8080/admin/devices/approve \
  -H 'Remote-User: admin@example.com' \
  -H 'Content-Type: application/json' \
  -d '{"endpoint_id":"receiver-finish-line","display_name":"Finish Line"}'
```

## Allow-list Distribution

Forwarders fetch the active receiver allow-list with the provisioning bearer
token:

```bash
curl -fsS http://127.0.0.1:8080/allowlist/receivers \
  -H "Authorization: Bearer ${THIN_NODE_PROVISIONING_TOKEN}"
```

Only active receivers appear in the response. Pending devices and forwarders are
excluded. If a receiver is revoked or no longer active, forwarders should stop
allowing new connections for that endpoint and close any open revoked connection
as their local allow-list refreshes.

## Announcer Push and Fenced Generation

Receivers take over a fenced announcer source generation before pushing rows:

```bash
curl -fsS -X POST http://127.0.0.1:8080/announcer/takeover \
  -H "Authorization: Bearer ${THIN_NODE_PROVISIONING_TOKEN}"
```

Each `POST /announcer/rows` request includes the returned
`announcer_source_generation`, `stream_id`, and monotonic `seq`. The thin node
rejects stale generations with `409 Conflict`, which prevents an older receiver
process from overwriting a newer source after restart or failover.

## Recovery Procedures

### Service restart

The thin node stores registry, allow-list, and announcer state in SQLite. On
restart, point `THIN_NODE_DB_PATH` at the same file and start the service again:

```bash
sudo systemctl restart rt-thin-node
curl -fsS http://127.0.0.1:8080/healthz
```

Forwarders and receivers retry registration, allow-list fetches, and announcer
pushes. No manual cursor migration is required.

### Database backup and restore

Stop the service before copying the SQLite file:

```bash
sudo systemctl stop rt-thin-node
sudo cp /var/lib/rusty-timer-thin-node/thin-node.sqlite3 /tmp/thin-node.sqlite3.bak
sudo systemctl start rt-thin-node
```

To restore, stop the service, replace the SQLite file with the backup, then
start the service. Devices may need to retry registration if the backup predates
their registration.

### Lost provisioning token

Rotate the token by changing `THIN_NODE_PROVISIONING_TOKEN` and restarting the
service. Existing device registry records remain in SQLite, but devices must be
configured with the new bearer token for future registration, allow-list fetch,
and announcer push requests.

## Troubleshooting

- `401 Unauthorized` on `/register`, `/allowlist/receivers`, or `/announcer/*`:
  check the `Authorization: Bearer` token exactly matches
  `THIN_NODE_PROVISIONING_TOKEN`.
- Empty allow-list: confirm the receiver registered and an admin approved it as
  active; pending receivers are intentionally excluded.
- `409 Conflict` on announcer row push: the receiver has a stale generation;
  retry takeover and resend durable rows with the newer generation.
- Admin approval does not work: verify Caddy + Authelia injected `Remote-User`
  and stripped any untrusted client-supplied copy.
