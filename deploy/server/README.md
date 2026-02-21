# Server Deployment (Docker Compose)

This is the canonical guide for deploying the `rt-server` + PostgreSQL stack with Docker
Compose.

- Forwarder deployment (Linux SBC + systemd): `deploy/systemd/` and
  `docs/runbooks/forwarder-operations.md`
- Receiver deployment (Windows app): `services/receiver/`

## What This Stack Includes

- `postgres` (`postgres:18-trixie`) for durable storage.
- `server` (`rt-server`) for:
  - WebSocket ingest (`/ws/v1/forwarders`)
  - WebSocket receiver delivery (`/ws/v1/receivers`)
  - REST API (`/api/v1/...`)
  - Dashboard static UI (`/`)

## Prerequisites

- Docker 24+ with BuildKit enabled.
- Docker Compose v2 (`docker compose`).
- Host networking/firewall configured for the exposed ports.
- Repository root available as Docker build context (`.`).

Optional but recommended:
- `docker buildx` for multi-arch images (for Docker Hub publishing).

## Compose Requirements

Files:
- `deploy/server/docker-compose.prod.yml`
- `deploy/server/.env` (copy from `deploy/server/.env.example`)

Required env value:
- `POSTGRES_PASSWORD`

Important optional env values:
- `SERVER_IMAGE` (default: `rt-server`)
- `SERVER_VERSION` (default: `latest`)
- `SERVER_PORT` (default: `8080`)
- `LOG_LEVEL` (default: `info`)

Create and edit env file:

```bash
cp deploy/server/.env.example deploy/server/.env
```

## Build The Server Image

Build from repository root so workspace-level files are available:

```bash
docker build -t rt-server:latest -f services/server/Dockerfile .
```

Versioned local build:

```bash
TAG=$(git rev-parse --short HEAD)
docker build -t rt-server:${TAG} -t rt-server:latest -f services/server/Dockerfile .
```

Then set `SERVER_IMAGE=rt-server` and `SERVER_VERSION=<tag>` in `deploy/server/.env`.

## Run With Docker Compose

Start:

```bash
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml up -d
```

Verify:

```bash
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml ps
curl -fsS http://localhost:8080/healthz
```

Logs:

```bash
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml logs -f server
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml logs -f postgres
```

Stop:

```bash
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml down
```

## Publish To Docker Hub (`iwismer/`)

Single-arch push:

```bash
docker login
docker build -t iwismer/rt-server:v0.1.0 -t iwismer/rt-server:latest -f services/server/Dockerfile .
docker push iwismer/rt-server:v0.1.0
docker push iwismer/rt-server:latest
```

Multi-arch push (`amd64` + `arm64`):

```bash
docker login
docker buildx create --name rt-builder --use 2>/dev/null || docker buildx use rt-builder
docker buildx build \
  --platform linux/amd64,linux/arm64 \
  -t iwismer/rt-server:v0.1.0 \
  -t iwismer/rt-server:latest \
  -f services/server/Dockerfile \
  --push .
```

To deploy a pushed image, set in `deploy/server/.env`:

```env
SERVER_IMAGE=iwismer/rt-server
SERVER_VERSION=v0.1.0
```

Then roll out:

```bash
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml pull server
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml up -d --no-deps server
```

## Update And Rollback

Update to a new tag:
1. Build/push the new tag.
2. Update `SERVER_VERSION` in `deploy/server/.env`.
3. Run `docker compose ... up -d --no-deps server`.

Rollback:
1. Set `SERVER_VERSION` back to the previous known-good tag.
2. Run `docker compose ... up -d --no-deps server`.

Postgres data stays in the `postgres_data` volume.

## Caddy + Authelia Example

`rt-server` does its own token auth for device WebSocket clients, but its HTTP/API endpoints
are not protected by built-in user auth. Do not expose it directly to the public internet.

Recommended pattern:
- Put Caddy in front of `rt-server`.
- Use Authelia forward-auth for dashboard + admin/API access.
- Bypass Authelia for device WebSocket paths (`/ws/v1/forwarders`,
  `/ws/v1/receivers`) so forwarders/receivers can still use bearer tokens.

Create an overlay compose file (example path: `deploy/server/docker-compose.edge.yml`):

```yaml
services:
  caddy:
    image: caddy:2.8
    container_name: rt-caddy
    restart: unless-stopped
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./caddy/Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy_data:/data
      - caddy_config:/config
    networks:
      - rt-public

  authelia:
    image: authelia/authelia:4
    container_name: rt-authelia
    restart: unless-stopped
    environment:
      TZ: UTC
      AUTHELIA_JWT_SECRET_FILE: /secrets/jwt_secret
      AUTHELIA_SESSION_SECRET_FILE: /secrets/session_secret
      AUTHELIA_STORAGE_ENCRYPTION_KEY_FILE: /secrets/storage_encryption_key
    volumes:
      - ./authelia/configuration.yml:/config/configuration.yml:ro
      - ./authelia/users_database.yml:/config/users_database.yml:ro
      - ./authelia/secrets:/secrets:ro
      - authelia_data:/var/lib/authelia
    networks:
      - rt-public

volumes:
  caddy_data:
  caddy_config:
  authelia_data:
```

Example Caddyfile:

```caddyfile
auth.example.com {
  reverse_proxy authelia:9091
}

timing.example.com {
  @device_ws path /ws/v1/forwarders /ws/v1/receivers
  handle @device_ws {
    reverse_proxy server:8080
  }

  handle {
    forward_auth authelia:9091 {
      uri /api/authz/forward-auth
      copy_headers Remote-User Remote-Groups Remote-Name Remote-Email
    }
    reverse_proxy server:8080
  }
}
```

Run with both files:

```bash
docker compose --env-file deploy/server/.env \
  -f deploy/server/docker-compose.prod.yml \
  -f deploy/server/docker-compose.edge.yml \
  up -d
```

## Token Provisioning (Forwarders/Receivers)

Use the admin API to create device tokens:

```bash
curl -sS -X POST http://localhost:8080/api/v1/admin/tokens \
  -H "Content-Type: application/json" \
  -d '{"device_type":"forwarder","device_id":"fwd-001"}'
```

The response includes the raw token once. Save it securely.

See `docs/runbooks/server-operations.md` for ongoing operations and recovery procedures.

## Documentation Checklist (Recommended Additions)

If you want full operator-ready docs, also include:

- Backup/restore runbook with RPO/RTO targets and restore test cadence.
- Secrets management policy (where Docker/Authelia secrets live and rotation cadence).
- Network exposure map (public ports, firewall rules, trusted source ranges).
- Token lifecycle policy (issuance, revocation, rotation, incident response).
- Observability setup (log retention, alerts, dashboard for queue/backlog/error rates).
- Upgrade policy (version pinning, rollout steps, rollback trigger criteria).
