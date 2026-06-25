# Receiver Operations Runbook

This runbook covers startup, recovery, and routine operations for the receiver
service and `receiver-headless` binary.

## Responsibilities

The receiver:

- Discovers allowed forwarders through the server.
- Connects directly to forwarders over iroh.
- Writes received events and cursors to local SQLite before acknowledging.
- Marks gaps when a forwarder reports that the requested cursor was pruned.
- Replays received events to local TCP ports for timing software.
- Writes idempotent DBF rows keyed by `(stream_id, seq)`.
- Optionally pushes sanitized announcer rows to the server with generation
  fencing and idempotency keys.

## Startup

1. Confirm the receiver data directory is writable.
2. Configure the server URL, receiver ID, and token in the receiver UI or
   control API. The token is a receiver enrollment voucher created from the
   Server UI `Admin` tab (**Receiver enrollment tokens**); its one-time secret
   is shown only once at creation.
3. Select streams by `forwarder_endpoint_id` and `stream_id`.
4. Start the desktop app or headless binary:

   ```bash
   receiver-headless --data-dir /var/lib/rusty-timer/receiver
   ```

5. Confirm local TCP ports are listening on loopback.
6. Confirm the server status board shows the receiver online.

## Recovery

### Reconnect/resume

The receiver resumes from durable cursors. If a process crashes, restart it and
verify that:

1. Existing subscriptions are loaded from SQLite.
2. Each P2P session resumes at the last acknowledged sequence.
3. DBF output has no duplicate `(stream_id, seq)` rows.

### Gap notice

If retention pruned the requested cursor, the receiver records a gap marker and
jumps to the cursor supplied by the forwarder. Operators should note the gap in
race records and use the forwarder journal backup if a full reconstruction is
needed.

### Local TCP replay

Timing software can reconnect to the receiver TCP port at any time. The receiver
replays durable received events for that subscription, then streams live reads.
If a timing program misses data, restart only the local timing connection before
restarting the receiver process.

### DBF output

DBF writes are idempotent. On restart, the receiver uses local received events
and the `(stream_id, seq)` key to avoid duplicate rows. If the configured DBF
path is unavailable, fix filesystem permissions and restart the receiver.

## Announcer Push

Announcer push is optional. The receiver publishes sanitized rows to the server
using a fenced generation. If an older receiver process is still running,
its writes are rejected by generation checks.

## Shutdown

Stop the receiver only after confirming timing software has flushed any local
reads it consumed:

```bash
pkill receiver-headless
```

A clean restart preserves subscriptions, cursors, received events, gap markers,
and DBF idempotency state.
