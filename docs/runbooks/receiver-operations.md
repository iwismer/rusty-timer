# Receiver Operations Runbook

This runbook covers operational procedures for the `rt-receiver` service
running on a Windows (or Linux) timing display/management workstation.

## Contents

1. [Service Overview](#service-overview)
2. [Startup and Installation](#startup-and-installation)
3. [Configuration](#configuration)
4. [Monitoring and Health](#monitoring-and-health)
5. [Recovery Procedures](#recovery-procedures)
6. [Stream Subscription Management](#stream-subscription-management)
7. [Exports (via Server)](#exports-via-server)
8. [Troubleshooting](#troubleshooting)

---

## Service Overview

The receiver is a client service that:
- Connects to the rt-server via WebSocket.
- Subscribes to one or more timing streams (identified by `forwarder_id + reader_ip`).
- Maintains a local SQLite cursor for durable resume on reconnect.
- Exposes a local control API and forwards events to the local timing display.

**Local durability**: The receiver uses SQLite with WAL+FULL sync. On startup,
it runs `PRAGMA integrity_check` and exits if it fails.

**At-least-once delivery**: Events may be delivered more than once (after
reconnect). The receiver deduplicates against its local cache.

---

## Startup and Installation

### Windows (standalone binary)

```powershell
# Create the data directory.
New-Item -ItemType Directory -Force "$env:LOCALAPPDATA\rusty-timer\receiver"

# Start the receiver.
.\rt-receiver.exe

# The receiver stores its SQLite profile at:
#   %LOCALAPPDATA%\rusty-timer\receiver\receiver.sqlite3
```

### Linux (systemd service, optional)

```bash
# Copy binary.
sudo cp rt-receiver /usr/local/bin/rt-receiver

# Start manually (or add to a systemd user service).
rt-receiver &
```

### Verify startup

```bash
# The receiver control API is always on port 9090.
curl http://localhost:9090/healthz
```

---

## Configuration

The receiver is configured via its SQLite profile database. On first run,
the receiver creates a default profile. Configuration is managed via the
control API or the receiver UI (Tauri desktop app, if applicable).

### Profile settings

| Setting | Description |
|---|---|
| Server URL | WebSocket URL of the rt-server |
| Bearer token | Auth token for this receiver device |
| Subscriptions | List of (forwarder_id, reader_ip) pairs to subscribe to |
| Local port | Override for the default local listener port |

### Port assignment

Default local port: `10000 + last_octet(reader_ip)`.
For `192.168.1.5`, the default port is `10005`.

If two subscribed streams have the same last octet, a manual port override
is required. Collisions are logged as errors at startup.

---

## Monitoring and Health

### Health endpoints

| Endpoint | Purpose |
|---|---|
| `GET /healthz` | Receiver process is alive. |
| `GET /readyz` | Receiver DB is open and integrity check passed. |

### Check connection status

```bash
# Via the control API.
curl http://localhost:9090/status
```

### Log monitoring

The receiver logs to stdout and to rolling local log files.

```bash
# On Linux — follow logs if running in terminal.
rt-receiver 2>&1 | tee receiver.log

# Check log file location (OS-specific data directory).
# Linux: ~/.local/share/rusty-timer/receiver/
# Windows: %LOCALAPPDATA%\rusty-timer\receiver\
```

---

## Recovery Procedures

### Receiver lost connection to server

The receiver reconnects automatically with an exponential backoff.
On reconnect, it sends its resume cursor (last acked seq per stream)
to the server, which replays any missed events.

```bash
# Check logs for reconnect attempts.
grep -i "reconnect\|resume\|cursor" receiver.log | tail -20
```

### Receiver integrity check failed at startup

If the receiver exits immediately with `FATAL: integrity_check failed`,
the local SQLite DB is corrupted.

**Recovery (data loss of local cache only — server DB is intact):**

```bash
# Linux:
rm ~/.local/share/rusty-timer/receiver/receiver.sqlite3

# Windows (PowerShell):
Remove-Item "$env:LOCALAPPDATA\rusty-timer\receiver\receiver.sqlite3"

# Restart the receiver — it will create a fresh profile.
# Re-subscribe to streams and it will replay from the server.
rt-receiver
```

The local profile (subscriptions, display labels) must be reconfigured.
All event data is still available on the server for replay.

### Resume from a specific cursor

If you need to re-receive events from a particular point in time:

1. Find the `stream_id` and `stream_epoch` from the server API.
2. Update the receiver cursor in the local SQLite DB directly:

```sql
-- Connect to the receiver SQLite DB.
-- Find the appropriate stream and reset cursor to desired seq.
UPDATE receiver_cursors
SET last_seq = <desired_seq - 1>
WHERE stream_id = '<stream_uuid>' AND stream_epoch = <epoch>;
```

After restarting the receiver, it will replay from `last_seq + 1`.

---

## Stream Subscription Management

### Receiver selection controls (`Mode` and `Epoch Scope`)

`Mode: Manual` is the default receiver selection mode.
- Operator chooses exactly which streams are subscribed.
- Stream changes occur only when the operator updates selections.

`Mode: Race` is an operator opt-in mode (not default).
- Enable it only when you want receiver selection to follow race context.
- In `Mode: Race`, set `Epoch Scope` to:
  - `Current` to replay only the current epoch.
  - `All` to replay all epochs available for the selected race.
- If behavior is unexpected, switch back to `Mode: Manual` and select streams explicitly.

### Stream list epoch visibility

Receiver UI stream rows now show the current `stream epoch` and current epoch name when available from the server. Use this to confirm the active stream context before subscribing or replaying.

### Subscribe to a new stream

The receiver subscribes during the hello handshake or mid-session
via the `receiver_subscribe` message.

Using the control API:

```bash
curl -X PUT http://localhost:9090/api/v1/subscriptions \
  -H "Content-Type: application/json" \
  -d '{"subscriptions": [{"forwarder_id": "fwd-001", "reader_ip": "192.168.1.100:10000", "local_port_override": null}]}'
```

Note: `PUT /api/v1/subscriptions` replaces the entire subscription list atomically.
To add a stream without removing existing ones, read the current list first with
`GET /api/v1/subscriptions`, then include all subscriptions in the PUT body.

Or restart the receiver with the updated profile to pick up new subscriptions.

### View current subscriptions

```bash
curl http://localhost:9090/api/v1/subscriptions
```

### Replay modes and targeted replay semantics

- Replay mode re-sends missed events according to cursor state for subscribed streams.
- Targeted replay is scoped to one selected stream context and should be used when replaying only a specific source.
- For targeted replay, verify the stream identity tuple before starting:
  - `forwarder_id`
  - `reader_ip`
  - `stream epoch`
- Do not start targeted replay until the stream row shows the expected epoch number/name.

---

## Exports (via Server)

Event exports are served by the rt-server, not the receiver.
See the [Server Operations Runbook](server-operations.md#exports) for export procedures.

The receiver has access to a local cache for display purposes only;
authoritative exports come from the server's Postgres database.

---

## Troubleshooting

| Symptom | Likely Cause | Action |
|---|---|---|
| Not receiving events | Not connected to server | Check server URL and token in profile |
| Integrity check failure | SQLite corruption | Delete local DB and restart |
| Port collision | Two streams with same last octet | Set `local_fallback_port` override |
| Duplicate events | Normal at-least-once behavior | Receiver deduplicates from local cache |
| Missing events after reconnect | Server replay working | Wait for replay to complete |
| Old events replaying | Cursor reset | Check cursor in local SQLite DB |
