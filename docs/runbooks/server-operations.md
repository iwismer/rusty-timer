# Server Operations Runbook

This runbook covers startup, recovery, and operator checks for the `server`
service in the P2P architecture. The server stores device registry state,
distributes the receiver allow-list to forwarders, and hosts the announcer row
status board.

## Service Overview

The server is a small Axum service backed by SQLite. It provides:

- `GET /healthz` and `GET /status` public read endpoints.
- TOFU device registration via `POST /register`.
- Receiver allow-list distribution via `GET /allowlist/receivers`.
- Fenced announcer push via `POST /announcer/takeover` and `POST /announcer/rows`.
- Admin approval via `POST /admin/devices/approve` and device rename via `POST
  /admin/devices/rename`, expected to sit behind Caddy + Authelia.
- Admin forwarder enrollment-token management via `/admin/enrollment-tokens`.

The service does not use `EndpointId` over plain HTTP as an authenticator. Device
routes use a provisioning bearer token or registered forwarder enrollment token;
admin identity is delegated to the reverse proxy.

## Startup and Installation

Install an arm64 release artifact on the server host:

```bash
sudo install -m 0755 server /usr/local/bin/server
sudo useradd -r -s /bin/false -m -d /var/lib/rusty-timer-server rt-server || true
sudo install -d -o rt-server -g rt-server -m 0750 /var/lib/rusty-timer-server
```

Set the required environment variables in the systemd unit or environment file:

```bash
BIND_ADDR=127.0.0.1:8080
SERVER_DB_PATH=/var/lib/rusty-timer-server/server.sqlite3
SERVER_PROVISIONING_TOKEN=<long random provisioning token>
LOG_LEVEL=info
```

Start manually for a smoke test:

```bash
sudo -u rt-server \
  BIND_ADDR=127.0.0.1:8080 \
  SERVER_DB_PATH=/var/lib/rusty-timer-server/server.sqlite3 \
  SERVER_PROVISIONING_TOKEN="$SERVER_PROVISIONING_TOKEN" \
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
| `POST /admin/devices/approve` | Admin only; Authelia must inject `Remote-User`. |
| `POST /admin/devices/rename` | Admin only; Authelia must inject `Remote-User`. |
| `/admin/enrollment-tokens*` | Admin only; Authelia must inject `Remote-User`. |
| `POST /register` | M2M/device `Authorization: Bearer <SERVER_PROVISIONING_TOKEN>`, or non-revoked forwarder enrollment token for forwarders. |
| `POST /forwarder/catalog` | M2M/device bearer token; accepts provisioning token or registered forwarder token. |
| `GET /allowlist/receivers` | M2M/device bearer token; accepts provisioning token or registered forwarder token. |
| `POST /announcer/takeover`, `POST /announcer/rows` | M2M/device bearer token; provisioning token only. |

Caddy must strip any inbound client-supplied `Remote-User` header before proxying
to the server. Only the trusted proxy may set that header after Authelia
admin authentication.

## Provisioning and Device Approval

Forwarders may be provisioned from the Server UI `SBC Setup` tab. Generate or
add a forwarder enrollment token, copy the one-time secret into the setup form,
download `user-data` and `network-config`, then boot the SBC. Generated token
secrets are shown only once; the token list exposes metadata only.

Forwarders and receivers may also self-register with the provisioning token:

```bash
curl -fsS -X POST http://127.0.0.1:8080/register \
  -H "Authorization: Bearer ${SERVER_PROVISIONING_TOKEN}" \
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

An already-approved device can be renamed at any time through the protected admin
route. The endpoint accepts the same body, leaves the approval state unchanged,
and rejects a blank display name with `400`:

```bash
curl -fsS -X POST http://127.0.0.1:8080/admin/devices/rename \
  -H 'Remote-User: admin@example.com' \
  -H 'Content-Type: application/json' \
  -d '{"endpoint_id":"receiver-finish-line","display_name":"Finish Tent"}'
```

## Enrollment Token Revocation

Revoking an unused enrollment token prevents first registration. Revoking a used
forwarder token blocks future per-device forwarder registration, catalog push,
and receiver allow-list requests that use that token. Revocation does not delete
the approved device row or remove the latest pushed forwarder catalog/status
snapshot; use separate device cleanup controls if the forwarder should be hidden
or decommissioned.

## Allow-list Distribution

Forwarders fetch the active receiver allow-list with the provisioning bearer
token or a registered non-revoked forwarder token:

```bash
curl -fsS http://127.0.0.1:8080/allowlist/receivers \
  -H "Authorization: Bearer ${SERVER_PROVISIONING_TOKEN}"
```

Only active receivers appear in the response. Pending devices and forwarders are
excluded. If a receiver is revoked or no longer active, forwarders should stop
allowing new connections for that endpoint and close any open revoked connection
as their local allow-list refreshes.

## Announcer Push and Fenced Generation

Receivers take over a fenced announcer source generation before pushing rows:

```bash
curl -fsS -X POST http://127.0.0.1:8080/announcer/takeover \
  -H "Authorization: Bearer ${SERVER_PROVISIONING_TOKEN}"
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

Forwarders and receivers retry registration, allow-list fetches, and announcer
pushes. No manual cursor migration is required.

### Database backup and restore

Stop the service before copying the SQLite file:

```bash
sudo systemctl stop rt-server
sudo cp /var/lib/rusty-timer-server/server.sqlite3 /tmp/server.sqlite3.bak
sudo systemctl start rt-server
```

To restore, stop the service, replace the SQLite file with the backup, then
start the service. Devices may need to retry registration if the backup predates
their registration.

### Lost provisioning token

Rotate the token by changing `SERVER_PROVISIONING_TOKEN` and restarting the
service. Existing device registry records remain in SQLite, but devices must be
configured with the new bearer token for future registration, allow-list fetch,
and announcer push requests.

## Troubleshooting

- `401 Unauthorized` on `/register`, `/forwarder/catalog`, or
  `/allowlist/receivers`: check whether the request uses the provisioning token
  or a registered non-revoked forwarder token. Receiver tokens do not authorize
  forwarder routes.
- `401 Unauthorized` on `/announcer/*`: check the `Authorization: Bearer` token
  exactly matches `SERVER_PROVISIONING_TOKEN`.
- Empty allow-list: confirm the receiver registered and an admin approved it as
  active; pending receivers are intentionally excluded.
- `409 Conflict` on announcer row push: the receiver has a stale generation;
  retry takeover and resend durable rows with the newer generation.
- Admin approval does not work: verify Caddy + Authelia injected `Remote-User`
  and stripped any untrusted client-supplied copy.
