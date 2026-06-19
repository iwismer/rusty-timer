# Receiver

The receiver discovers allowed forwarders through the server, connects
directly over iroh, durably stores received events/cursors/gaps in SQLite, and
re-exposes subscribed streams as local TCP ports for timing software.

## Build

```bash
cargo build --release -p receiver --bin receiver-headless
```

The desktop app is built through the receiver UI/Tauri workspace:

```bash
cd apps/receiver-ui
npm run build
cargo tauri build
```

## Runtime Model

- `receiver-headless` hosts the control API and P2P runtime without Tauri.
- The desktop app uses the same command registry as the headless control bridge.
- The `test-bridge` HTTP surface is loopback-only, feature-gated, and absent in
  release builds.
- Acknowledgements are sent only after events and cursors are durably written.
- DBF output is idempotent by `(stream_id, seq)`.

## Local TCP Output

Each subscription maps a `(forwarder_endpoint_id, stream_id)` pair to a local
TCP port. Timing software connects to that local port and receives replayed
stored events followed by live reads.

## Operations

See [Receiver operations](../../docs/runbooks/receiver-operations.md) for
startup, reconnect/resume, gap handling, DBF output, and announcer push.
