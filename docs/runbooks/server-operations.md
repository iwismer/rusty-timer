# Server Operations Runbook

This runbook covers operational procedures for the `rt-server` service
and its PostgreSQL database.

## Contents

1. [Service Overview](#service-overview)
2. [Startup and Deployment](#startup-and-deployment)
3. [Configuration](#configuration)
4. [Monitoring and Health](#monitoring-and-health)
5. [Recovery Procedures](#recovery-procedures)
6. [Epoch Reset (Admin)](#epoch-reset-admin)
7. [Exports](#exports)
8. [Manual Retention Delete (DB-Admin Only)](#manual-retention-delete-db-admin-only)
9. [Troubleshooting](#troubleshooting)

---

## Service Overview

The rt-server is a stateless Rust service backed by PostgreSQL. It:
- Accepts forwarder WebSocket connections and persists read events.
- Delivers events to receiver WebSocket clients with cursor-based replay.
- Exposes a REST API for stream management, metrics, and exports.
- Serves the dashboard UI as static files.

**Server is stateless** — all event data lives in Postgres. Restarting
the server process does not lose any data.

---

## Startup and Deployment

### Production deployment (Docker Compose)

```bash
# Copy and edit the environment file.
cp deploy/server/.env.example deploy/server/.env
# Edit deploy/server/.env: set POSTGRES_PASSWORD and image/tag values.

# Start the stack.
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml up -d

# Check status.
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml ps
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml logs server --tail=50
```

See `deploy/server/README.md` for image build/publish and reverse-proxy examples.

### First-time startup

On first startup, the server automatically applies the database migration:
```
migrations/0001_init.sql
```

This creates the required tables: `device_tokens`, `streams`, `events`,
`stream_metrics`, and `receiver_cursors`.

### Provision device tokens

After migration, provision forwarder and receiver tokens:

```bash
# Create a forwarder token
curl -sS -X POST http://localhost:8080/api/v1/admin/tokens \
  -H "Content-Type: application/json" \
  -d '{"device_type":"forwarder","device_id":"fwd-001"}'

# Create a receiver token
curl -sS -X POST http://localhost:8080/api/v1/admin/tokens \
  -H "Content-Type: application/json" \
  -d '{"device_type":"receiver","device_id":"receiver-001"}'
```

The response includes the raw token once. Store it in your secret manager and
distribute it to the corresponding device.

### Verify startup

```bash
curl http://localhost:8080/healthz
curl http://localhost:8080/readyz

# List streams (should be empty on first start).
curl http://localhost:8080/api/v1/streams
```

---

## Configuration

The server is configured via environment variables (12-factor model):

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | (required) | PostgreSQL connection string |
| `BIND_ADDR` | `0.0.0.0:8080` | HTTP/WS listen address |
| `LOG_LEVEL` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error` |

No TOML config file — all config from environment.

---

## Monitoring and Health

### Health endpoints

| Endpoint | Purpose |
|---|---|
| `GET /healthz` | Server is alive. |
| `GET /readyz` | Readiness endpoint (currently same behavior as `/healthz`). |

### Log monitoring (Docker)

```bash
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml logs -f server
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml logs -f postgres
```

### Stream status API

```bash
# List all streams and their online status.
curl http://localhost:8080/api/v1/streams | jq '.streams[] | {forwarder_id, reader_ip, online, stream_epoch}'

# Get metrics for a specific stream.
curl http://localhost:8080/api/v1/streams/{stream_id}/metrics | jq .
```

---

## Recovery Procedures

### Server restart (no data loss)

Since the server is stateless, a restart is safe:

```bash
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml restart server

# Or for a full stack restart:
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml down && \
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml up -d
```

After restart, connected forwarders and receivers will reconnect automatically.
Any in-flight events are re-sent by the forwarder (at-least-once delivery).

### Database recovery

If Postgres fails:

```bash
# Check Postgres health.
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml logs postgres --tail=50

# Restart Postgres.
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml restart postgres

# Wait for Postgres to become healthy before restarting server.
docker compose --env-file deploy/server/.env -f deploy/server/docker-compose.prod.yml up -d server
```

### Postgres data directory backup

```bash
# Backup using pg_dump (recommended for logical backups).
docker exec rt-postgres pg_dump -U rtserver rtserver > backup-$(date +%Y%m%d).sql

# Restore from backup.
docker exec -i rt-postgres psql -U rtserver rtserver < backup-YYYYMMDD.sql
```

---

## Epoch Reset (Admin)

An epoch reset re-sequences events on a specific reader stream.
Used when race epoch boundaries change (e.g. chip list rollover).

```bash
# List streams to find the stream_id.
curl http://localhost:8080/api/v1/streams | jq '.streams[] | {stream_id, forwarder_id, reader_ip}'

# Trigger epoch reset (requires the forwarder to be connected).
curl -X POST http://localhost:8080/api/v1/streams/{stream_id}/reset-epoch

# If forwarder is not connected, returns HTTP 409.
# Connect the forwarder first, then retry.
```

After reset:
- `stream_epoch` increments by 1.
- New events from the forwarder start at `seq=1` in the new epoch.
- Unacked events from the old epoch remain replayable until drained.

---

## Exports

Exports are available per-stream in two formats:
- **RAW**: all unique read lines in canonical order.
- **CSV**: deduplicated events in CSV format.

Both exports cover **deduplicated (canonical) events only** — retransmits
are excluded.

```bash
# Export raw reads for a stream.
curl http://localhost:8080/api/v1/streams/{stream_id}/export.txt \
  -o export-raw-$(date +%Y%m%d).txt

# Export CSV for a stream.
curl http://localhost:8080/api/v1/streams/{stream_id}/export.csv \
  -o export-$(date +%Y%m%d).csv
```

### Export all streams

```bash
# Iterate over all streams and export each.
curl -s http://localhost:8080/api/v1/streams | jq -r '.streams[].stream_id' | \
while read stream_id; do
    echo "Exporting stream ${stream_id}..."
    curl -s "http://localhost:8080/api/v1/streams/${stream_id}/export.csv" \
        -o "export-${stream_id}-$(date +%Y%m%d).csv"
done
```

---

## Manual Retention Delete (DB-Admin Only)

**WARNING: This operation permanently deletes event data and cannot be undone.
It is restricted to database administrators only. There is no public delete API.**

In v1, the server keeps all events indefinitely. Manual delete is a
DB-admin runbook procedure only.

### Before deleting

1. Confirm the export has been taken (see Exports section above).
2. Confirm with the system operator that the data is no longer needed.
3. Ensure no receivers are actively consuming the stream.

### Procedure

Connect to the Postgres database as an admin user:

```bash
docker exec -it rt-postgres psql -U rtserver rtserver
```

Then execute the delete:

```sql
-- Find the stream(s) to delete.
SELECT stream_id, forwarder_id, reader_ip, stream_epoch
FROM streams
WHERE forwarder_id = 'fwd-001' AND reader_ip = '192.168.1.100';

-- Delete events for a specific stream (DB-admin only).
-- Replace {stream_id} with the UUID from the above query.
BEGIN;

-- Delete receiver cursors for this stream first.
DELETE FROM receiver_cursors WHERE stream_id = '{stream_id}';

-- Delete stream metrics.
DELETE FROM stream_metrics WHERE stream_id = '{stream_id}';

-- Delete events.
DELETE FROM events WHERE stream_id = '{stream_id}';

-- Optionally delete the stream record itself.
-- Only do this if the forwarder will not reconnect to this stream.
-- DELETE FROM streams WHERE stream_id = '{stream_id}';

COMMIT;

-- Verify.
SELECT COUNT(*) FROM events WHERE stream_id = '{stream_id}';
```

**This operation is irreversible. Exports should be taken before deletion.**

---

## Troubleshooting

| Symptom | Likely Cause | Action |
|---|---|---|
| `/readyz` 503 | DB not connected | Check `DATABASE_URL`, check Postgres health |
| Forwarder not connecting | Wrong token | Verify token hash in `device_tokens` |
| Events not reaching receivers | Receiver not subscribed | Check receiver hello resume cursors |
| `INTEGRITY_CONFLICT` in logs | Mismatched event payloads | Investigate forwarder re-keying; contact operator |
| DB migration failed | Wrong DB permissions | Ensure `DATABASE_URL` user has `CREATE TABLE` permissions |
| High receiver backlog | Slow receiver | Check receiver connection and ack rate |
