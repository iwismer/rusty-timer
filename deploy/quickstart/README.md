# Quickstart: Evaluate Rusty Timer with Docker

Run the Rusty Timer server and dashboard locally with a single command.
No Rust toolchain or build step required.

## Prerequisites

- Docker 24+ with Docker Compose v2

## Start

From the repository root:

```bash
docker compose -f deploy/quickstart/docker-compose.yml up -d
```

Wait a few seconds for Postgres to initialise and the server to run
migrations, then verify:

```bash
curl http://localhost:8080/healthz
# ok
```

Open **http://localhost:8080** in a browser to see the dashboard.

## Create Device Tokens

Before connecting a forwarder or receiver you need to create tokens.

### Forwarder token

```bash
curl -sS -X POST http://localhost:8080/api/v1/admin/tokens \
  -H 'Content-Type: application/json' \
  -d '{"device_type": "forwarder", "device_id": "fwd-001"}'
```

Save the raw token from the response — it will not be shown again.

### Receiver token

```bash
curl -sS -X POST http://localhost:8080/api/v1/admin/tokens \
  -H 'Content-Type: application/json' \
  -d '{"device_type": "receiver", "device_id": "receiver-001"}'
```

## Connect a Forwarder

Download a pre-built forwarder binary from the
[Releases](https://github.com/iwismer/rusty-timer/releases) page, then
point it at this server using its TOML config file. See the
[forwarder configuration reference](../../services/forwarder/README.md#configuration)
for the config format, or [deploy/sbc/](../sbc/) for Raspberry Pi
deployment.

Set `server.base_url` to `ws://YOUR_HOST_IP:8080` and put the raw token
in the file referenced by `auth.token_file`.

## Connect a Receiver

Download a pre-built receiver binary from the
[Releases](https://github.com/iwismer/rusty-timer/releases) page. On
Windows, see the
[receiver quickstart](../../docs/receiver-quickstart.md) for a
step-by-step walkthrough, or the
[receiver operations runbook](../../docs/runbooks/receiver-operations.md)
for full operational procedures.

Set the server URL to `ws://YOUR_HOST_IP:8080` and paste the raw token
you created above.

## URLs

| Page | URL |
|------|-----|
| Dashboard | http://localhost:8080 |
| Announcer config | http://localhost:8080/announcer-config |
| Announcer screen | http://localhost:8080/announcer |
| Health check | http://localhost:8080/healthz |
| API base | http://localhost:8080/api/v1/streams |

## Stop

```bash
docker compose -f deploy/quickstart/docker-compose.yml down
```

To also remove the Postgres data volume (full reset):

```bash
docker compose -f deploy/quickstart/docker-compose.yml down -v
```

## Next Steps

- [deploy/server/](../server/) — Production server deployment with TLS
  and backups
- [deploy/sbc/](../sbc/) — Forwarder on Raspberry Pi
- [docs/runbooks/race-day-operator-guide.md](../../docs/runbooks/race-day-operator-guide.md)
  — Race-day operations
