# Server Docker Deployment

This directory contains a sample Docker Compose deployment for the Rusty Timer
coordination server. It runs:

- `server` — the SQLite-backed registry/status/announcer service with the
  embedded server UI.
- `caddy` — the public reverse proxy that separates public, device-token, and
  admin/browser routes.

The chip-read data plane remains direct forwarder-to-receiver P2P. The server
coordinates registration, receiver allow-list distribution, forwarder discovery,
and announcer/status state.

Published server releases currently provide an `aarch64-unknown-linux-gnu`
standalone binary and a `linux/amd64` Docker image. Use the Docker image for an
x86_64/amd64 container host.

## Files

- `docker-compose.yml` — sample server + Caddy deployment.
- `.env.example` — environment values to copy and edit.
- `Caddyfile.example` — route protection example.

## Quick start

From the repository root:

```bash
cp deploy/server/.env.example deploy/server/.env
# Edit deploy/server/.env for your hostname, image tag, and Authelia URL.

docker compose --env-file deploy/server/.env \
  -f deploy/server/docker-compose.yml \
  build server

docker compose --env-file deploy/server/.env \
  -f deploy/server/docker-compose.yml \
  up -d
```

Verify:

```bash
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.yml ps
curl -fsS http://localhost/healthz
curl -fsS http://localhost/status
```

If you deploy a published image instead of building locally, set:

```env
SERVER_IMAGE=iwismer/rt-server
SERVER_VERSION=v0.1.0
```

Then run:

```bash
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.yml pull server
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.yml up -d
```

Do not run `docker compose build server` for a published-image deployment; the
Compose file includes a local `build:` stanza only for development builds from a
checkout.

## Caddy route policy

`Caddyfile.example` uses three route groups:

| Routes | Auth at Caddy | Auth in server | Notes |
| --- | --- | --- | --- |
| `GET /healthz`, `GET /status` | none | none | Public health and operational status. `/status` includes device, forwarder, stream, and announcer metadata; protect it in Caddy if that should not be public for your deployment. |
| `POST /register`, `POST /forwarder/catalog`, `GET /forwarders`, `GET /allowlist/receivers`, `POST /announcer/rows`, `POST /announcer/takeover` | none | bearer device token | Devices must not need a browser session. |
| `/admin/*` and embedded UI fallback | Authelia `forward_auth` | `Remote-User` for `/admin/*` | Caddy strips spoofable `Remote-*` headers before auth. |

The Compose file sets `SERVER_TRUSTED_PROXY=1` for the server container. Only do
this when the server is reachable exclusively through a header-stripping trusted
proxy such as the sample Caddy service. Do not publish the server container port
directly to the host in this mode.

## Authelia

This sample does not run Authelia. Point `AUTHELIA_UPSTREAM` at an existing
Authelia base URL/origin, or add Authelia to the same Docker network. Caddy must
be able to call:

```text
${AUTHELIA_UPSTREAM}/api/authz/forward-auth
```

Caddy copies `Remote-User` to the server only after Authelia authenticates the
request. The server denies `/admin/*` when `SERVER_TRUSTED_PROXY` is unset.

## Data and backups

The server stores SQLite state in the `server_data` Docker volume at:

```text
/var/lib/rusty-timer-server/server.sqlite3
```

Back up the database by stopping the server and copying the SQLite file from the
volume, or by using your host's volume backup tooling. Restores should preserve
file ownership for the `rt-server` user inside the container.

## Device provisioning flow

1. Sign in through the protected server UI.
2. Create enrollment vouchers for each forwarder and receiver.
3. Configure the forwarder/receiver with its voucher and server URL.
4. The device registers once and receives a server-minted per-device token.
5. Approve the pending device in the server UI.

Forwarders then push their stream catalog and fetch the active receiver
allow-list. Receivers discover approved forwarders through `/forwarders` and can
push announcer rows after approval.

## Updating

```bash
# Change SERVER_VERSION in deploy/server/.env, then:
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.yml pull server
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.yml up -d --no-deps server
```

Rollback by setting `SERVER_VERSION` back to the previous known-good tag and
running the same `up -d --no-deps server` command.
