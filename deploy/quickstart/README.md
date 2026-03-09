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
curl http://localhost:8080/api/healthz
# {"status":"ok"}
```

Open **http://localhost:8080** in a browser to see the dashboard.

## Create Device Tokens

Before connecting a forwarder or receiver you need to create tokens.

### Forwarder token

```bash
curl -s -X POST http://localhost:8080/api/device-tokens \
  -H 'Content-Type: application/json' \
  -d '{"name": "my-forwarder", "role": "forwarder"}' | jq .
```

Save the `raw_token` value from the response — it will not be shown
again.

### Receiver token

```bash
curl -s -X POST http://localhost:8080/api/device-tokens \
  -H 'Content-Type: application/json' \
  -d '{"name": "my-receiver", "role": "receiver"}' | jq .
```

## Connect a Forwarder

Download a pre-built forwarder binary from the
[Releases](https://github.com/iwismer/rusty-timer/releases) page, then
point it at this server using its TOML config file. See
[deploy/sbc/](../sbc/) for the full forwarder deployment guide.

Set the server URL to `ws://YOUR_HOST_IP:8080/ws/forwarder` and paste
the raw token you created above.

## Connect a Receiver

Download a pre-built receiver binary from the
[Releases](https://github.com/iwismer/rusty-timer/releases) page. See
[docs/runbooks/receiver-operations.md](../../docs/runbooks/receiver-operations.md)
for configuration and usage.

Set the server URL to `ws://YOUR_HOST_IP:8080/ws/receiver` and paste
the raw token you created above.

## URLs

| Page | URL |
|------|-----|
| Dashboard | http://localhost:8080 |
| Announcer config | http://localhost:8080/announcer/config |
| Announcer screen | http://localhost:8080/announcer |
| Health check | http://localhost:8080/api/healthz |
| API base | http://localhost:8080/api/ |

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
