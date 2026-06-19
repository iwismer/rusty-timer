# Forwarder Operations Runbook

This runbook covers startup, recovery, and routine operations for the
`forwarder` service in the P2P architecture.

## Responsibilities

The forwarder:

- Reads IPICO TCP streams from one or more physical readers.
- Appends every read to the local SQLite journal before publishing.
- Serves typed iroh control/data streams to allowed receivers.
- Keeps `stream_id` stable for a physical reader and keeps sequence numbers
  monotonic across epoch-name changes.
- Applies retention rules and emits explicit gap notices when a receiver asks
  for pruned data.

## Startup

1. Confirm the reader is reachable from the SBC LAN.
2. Confirm `forwarder.toml` points at the local reader address and the intended
   data directory.
3. Confirm the server URL and M2M token are configured for registration and
   allow-list polling.
4. Start or restart the service:

   ```bash
   sudo systemctl restart rt-forwarder
   sudo systemctl status rt-forwarder
   ```

5. Check status health:

   ```bash
   curl -fsS http://127.0.0.1:8080/healthz
   curl -fsS http://127.0.0.1:8080/readyz
   ```

6. Verify the server status board shows the endpoint online.

## Recovery

### Reader disconnected

1. Check reader power and network link.
2. Confirm the configured reader address is still reachable.
3. Restart the reader task through the forwarder UI or restart the service.
4. Verify new reads append to the same stream with the next sequence number.

### Receiver cannot subscribe

1. Check the server allow-list for the receiver endpoint ID.
2. Confirm the forwarder has fetched the latest allow-list generation.
3. If a receiver was revoked, confirm any existing connection was force-closed.
4. Reconnect the receiver so it fetches the current endpoint address.

### Journal disk pressure

1. Check free disk space on the data partition.
2. Inspect retention metrics from the forwarder status UI.
3. If necessary, increase available storage or reduce retention thresholds.
4. Manual clear is allowed only when operators accept that receivers behind the
   retained cursor will receive gap notices. Manual clear must not reset the
   next sequence number.

## Epoch Operations

An epoch-name update is metadata only. It creates a new stream epoch while the
same `stream_id` continues and sequence numbers remain monotonic.

Use epoch updates for race boundaries, timing point resets, or operator-visible
labels. Do not create a new `stream_id` unless the journal is lost and the
forwarder cannot restore the previous `next_seq` and epoch metadata.

## Shutdown

```bash
sudo systemctl stop rt-forwarder
```

A clean shutdown stops reader tasks, keeps the SQLite journal intact, and allows
receivers to resume from durable cursors after restart.
