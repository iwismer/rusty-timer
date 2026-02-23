# Forwarder Operations Runbook

This runbook covers operational procedures for the `rt-forwarder` service
running on a Linux timing system (Raspberry Pi or similar).

## Contents

1. [Service Overview](#service-overview)
2. [Startup and Installation](#startup-and-installation)
3. [Configuration](#configuration)
4. [Monitoring and Health](#monitoring-and-health)
5. [Recovery Procedures](#recovery-procedures)
6. [Epoch Reset](#epoch-reset)
7. [Journal Retention Management](#journal-retention-management)
8. [Troubleshooting](#troubleshooting)

---

## Service Overview

The forwarder reads from IPICO timing readers, journals events locally
to SQLite (power-loss-safe: WAL + synchronous=FULL), and forwards them
to the rt-server over an authenticated WebSocket connection.

Key properties:
- **At-least-once delivery**: events are retransmitted until the server acks.
- **Guaranteed backfill target**: 24 hours offline.
- **Local durability**: SQLite with WAL+FULL sync; integrity check on startup.
- **Config source**: TOML file only (`/etc/rusty-timer/forwarder.toml`).

---

## Startup and Installation

### Install as a systemd service

```bash
# Copy the binary.
sudo cp /path/to/rt-forwarder /usr/local/bin/rt-forwarder
sudo chmod +x /usr/local/bin/rt-forwarder

# Create service user.
sudo useradd -r -s /bin/false -m -d /var/lib/rusty-timer rt-forwarder

# Create config and data directories.
sudo mkdir -p /etc/rusty-timer /var/lib/rusty-timer
sudo chown rt-forwarder:rt-forwarder /var/lib/rusty-timer

# Copy the systemd unit.
sudo cp deploy/systemd/rt-forwarder.service /etc/systemd/system/

# Enable and start.
sudo systemctl daemon-reload
sudo systemctl enable rt-forwarder
sudo systemctl start rt-forwarder

# Verify startup.
sudo systemctl status rt-forwarder
journalctl -u rt-forwarder -f
```

### Verify it is running

```bash
# Check systemd status.
sudo systemctl status rt-forwarder

# Check the status HTTP endpoint (if reachable on the local network).
curl http://localhost:8080/healthz
curl http://localhost:8080/readyz
```

---

## Configuration

The forwarder reads from `/etc/rusty-timer/forwarder.toml`.
No environment variable overrides are supported; TOML is the sole source.

### Minimum required fields

```toml
schema_version = 1

[server]
base_url = "wss://timing.example.com"

[auth]
token_file = "/etc/rusty-timer/forwarder.token"

[[readers]]
target = "192.168.1.100"
read_type = "raw"
enabled = true
```

### Token file

The `auth.token_file` must contain a single line with the Bearer token
issued by the server operator. This token is never logged.

### Device power-action controls

The forwarder config supports an optional control section:

```toml
[control]
allow_power_actions = false
```

- `allow_power_actions = false` (default): device restart/shutdown API/UI actions are blocked.
- `allow_power_actions = true`: enables `restart-device` and `shutdown-device` control actions.

SBC provisioning via `deploy/sbc/rt-setup.sh` sets this to `true` by default.
The setup script installs
`/etc/polkit-1/rules.d/90-rt-forwarder-power-actions.rules` when
`[control].allow_power_actions = true`, and removes it when
`[control].allow_power_actions = false`.

---

## Monitoring and Health

### Log monitoring

```bash
# Follow live logs.
journalctl -u rt-forwarder -f

# Last 100 lines.
journalctl -u rt-forwarder -n 100

# Filter for errors.
journalctl -u rt-forwarder -p err
```

### Health endpoints

| Endpoint | Purpose |
|---|---|
| `GET /healthz` | Service is alive (returns `ok`). |
| `GET /readyz` | Service is ready: config, storage, and worker loops initialized. Note: does NOT require active uplink connectivity. |

---

## Recovery Procedures

### Forwarder lost connection to server

The forwarder reconnects automatically. Check logs for reconnect attempts:

```bash
journalctl -u rt-forwarder -n 50 | grep -i "reconnect\|uplink\|error"
```

If the server is unreachable for an extended period, the forwarder
continues buffering events to the local SQLite journal until the 24-hour
retention window. Events older than the watermark may be pruned if disk
pressure exceeds the configured threshold (default: 80%).

### Forwarder was killed (power loss)

On restart, the forwarder:
1. Opens the SQLite journal.
2. Runs `PRAGMA integrity_check` — exits with error if it fails.
3. Replays all unacked events to the server from the last acked cursor.

No manual intervention required. Check logs after restart:

```bash
journalctl -u rt-forwarder -n 100 | grep -i "reopen\|integrity\|replay\|unacked"
```

### Safe shutdown before unplugging

Before unplugging an SBC forwarder, issue a clean shutdown first (UI action
or `sudo systemctl poweroff`). This avoids journal corruption and reduces risk
of losing locally buffered events.

### SQLite integrity check failed at startup

If the forwarder exits immediately with `FATAL: integrity_check failed`, the
local journal is corrupted. This should not happen under normal operation
(WAL+FULL sync prevents this), but can occur after hardware failure.

**Recovery (DB-admin procedure):**

```bash
# Stop the service.
sudo systemctl stop rt-forwarder

# Backup the corrupt journal.
sudo cp /var/lib/rusty-timer/forwarder.sqlite3 /tmp/forwarder.sqlite3.bak

# Remove the corrupt journal (events buffered since last server ack are lost,
# but all events acked by the server are already in the server DB).
sudo rm /var/lib/rusty-timer/forwarder.sqlite3

# Restart — the forwarder will create a fresh journal.
sudo systemctl start rt-forwarder
```

---

## Epoch Reset

An epoch reset is used when a race-epoch boundary occurs (e.g. chip IDs
from a previous event must be excluded from a new epoch).

### Current epoch name controls (UI)

Forwarder UI supports setting and clearing the current epoch name for the
active stream context.

Operator procedure:
1. Open the stream in the Forwarder UI.
2. In the active stream context, set `Current epoch name` and click `Save`.
3. To remove it, click `Clear`.

Behavior:
- This updates epoch labeling for operators and receiver UI visibility.
- This does not reset sequence numbers.
- This does not increment `stream epoch`.

### Triggering an epoch reset

The server operator triggers the reset via the server HTTP API:

```bash
# Replace {stream_id} with the UUID from GET /api/v1/streams.
curl -X POST https://timing.example.com/api/v1/streams/{stream_id}/reset-epoch \
  -H "Authorization: Bearer <admin-token>"
```

The server sends an `epoch_reset_command` to the connected forwarder.
The forwarder:
1. Increments `stream_epoch` for the affected reader.
2. Restarts `seq` at 1 for new events.
3. Applies the new epoch before sending subsequent events for that reader.
4. **Does NOT discard unacked events from the old epoch** — they remain
   replayable until the server acks them.

If the forwarder is not connected when the reset is requested, the server
returns HTTP 409. Reconnect the forwarder and retry.

### Verifying epoch reset

```bash
# Check the stream epoch in the server API.
curl https://timing.example.com/api/v1/streams | jq '.streams[] | {forwarder_id, reader_ip, stream_epoch, current_epoch_name}'
```

---

## Journal Retention Management

The forwarder journal prunes **acked events first** when disk usage exceeds
the configured watermark (default: 80% of the SQLite file size limit).

Unacked events are only pruned when all acked events are exhausted AND disk
pressure still requires it. In this case, the forwarder logs a **degraded
retention state** warning.

### Check journal size

```bash
ls -lh /var/lib/rusty-timer/forwarder.sqlite3
```

### Adjust watermark threshold

Edit `/etc/rusty-timer/forwarder.toml`:

```toml
[journal]
prune_watermark_pct = 80   # default; lower to prune earlier
```

Restart the service after config changes:

```bash
sudo systemctl restart rt-forwarder
```

---

## Troubleshooting

| Symptom | Likely Cause | Action |
|---|---|---|
| `/readyz` returns non-200 | Config load or storage error | Check logs: `journalctl -u rt-forwarder -n 50` |
| No events reaching server | Network/auth issue | Check `server.base_url` and token file |
| "session already active" in logs | Duplicate forwarder connection | Check for multiple forwarder processes: `pgrep rt-forwarder` |
| Events backlogged but not sent | Server unreachable | Normal; forwarder will replay when server reconnects |
| High disk usage warning | Slow ack rate / high event volume | Check server connectivity; consider lower `prune_watermark_pct` |
| "Interactive authentication required" from restart/shutdown controls | Missing or misconfigured polkit rule for `rt-forwarder` | Re-run `sudo bash deploy/sbc/rt-setup.sh` (installs `/etc/polkit-1/rules.d/90-rt-forwarder-power-actions.rules` when enabled), then restart `rt-forwarder` |
