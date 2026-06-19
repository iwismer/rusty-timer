# Rusty Timer

[![Rust CI](https://github.com/iwismer/rusty-timer/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/iwismer/rusty-timer/actions/workflows/ci.yml)
[![UI CI](https://github.com/iwismer/rusty-timer/actions/workflows/ui.yml/badge.svg?branch=main)](https://github.com/iwismer/rusty-timer/actions/workflows/ui.yml)
[![Embed UI CI](https://github.com/iwismer/rusty-timer/actions/workflows/embed-ui.yml/badge.svg?branch=main)](https://github.com/iwismer/rusty-timer/actions/workflows/embed-ui.yml)

Rusty Timer forwards IPICO chip-timing reads over the internet so timing
software does not need a direct cable to each reader. It is designed for
remote timing points, multi-site races, and backup paths for local reads.

It is compatible with timing software that accepts IPICO TCP streams
(tested with IPICO Connect).

## How It Works

```
             ┌─── Field ───┐        ┌── Coordination ─┐        ┌── Timing Tent ──┐
IPICO Reader ──TCP──► Forwarder ──iroh/P2P──► Server ◄──HTTP── Receiver ──TCP──► Timing Software
                         │                 (registry,
                   SQLite journal       allow-list,
                  + replay cursor       status board)
```

The **[Forwarder](services/forwarder/)** runs next to each IPICO reader.
It journals every read to local SQLite, exposes a typed P2P control/data
plane over iroh, and keeps per-stream sequence numbers monotonic across
epoch changes.

The **[Server](services/server/)** is the lightweight coordination
service. It stores endpoint registrations, distributes allow-lists,
receives announcer/status updates, and exposes the status board. It does
not carry chip-read data.

The **[Receiver](services/receiver/)** connects directly to forwarders
through iroh, durably stores received events and cursors, acknowledges
only after durable writes, and replays subscribed streams as local TCP
ports for existing timing software.

If a link drops, reads remain safe: the forwarder journal is the source
of truth, receivers resume from durable cursors, and pruned cursors are
reported as explicit gap markers.

## Other Components

- **[Streamer](services/streamer/)** connects to IPICO readers and fans
  out local TCP streams without the remote forwarding stack.
- **[Emulator](services/emulator/)** simulates IPICO readers for local
  development and deterministic tests.
- **[Forwarder UI](apps/forwarder-ui/)** and
  **[Receiver UI](apps/receiver-ui/)** are SvelteKit frontends embedded
  in their service binaries.
- **[Shared UI](apps/shared-ui/)** contains reusable UI components and
  validation logic.

## Compatibility

- **Readers:** tested with IPICO Lite readers; intended for IPICO Elite
  and Super Elite readers as well.
- **Timing software:** any software that accepts IPICO TCP streams.
- **Forwarder hardware:** Raspberry Pi 3/4/5 with a 64-bit OS, or any
  Linux SBC/server with an ARM64 or x86-64 CPU.

## Quick Demo

Run the deterministic loopback P2P stack locally with simulated readers:

```bash
uv run scripts/e2e/run_stack.py
```

The loopback stack starts the emulator, forwarder, receiver-headless,
and server with relay/discovery disabled and injected local addresses.
See [scripts/README.md](scripts/README.md) for local development commands.

## Deploying for Real

| Component | Guide |
|-----------|-------|
| Forwarder on Raspberry Pi | [deploy/sbc/](deploy/sbc/) |
| Race-day operations | [docs/runbooks/race-day-operator-guide.md](docs/runbooks/race-day-operator-guide.md) |
| Forwarder operations | [docs/runbooks/forwarder-operations.md](docs/runbooks/forwarder-operations.md) |
| Receiver operations | [docs/runbooks/receiver-operations.md](docs/runbooks/receiver-operations.md) |
| Server operations | [docs/runbooks/server-operations.md](docs/runbooks/server-operations.md) |

Pre-built binaries are available on the [Releases](https://github.com/iwismer/rusty-timer/releases) page.

See the [full documentation index](docs/) for all guides, runbooks, and
reference docs.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for building from source, running
tests, and code quality checks.

## Licence

GPL-3.0 — see [LICENCE.txt](LICENCE.txt)
