# Rusty Timer

[![Rust CI](https://github.com/iwismer/rusty-timer/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/iwismer/rusty-timer/actions/workflows/ci.yml)
[![UI CI](https://github.com/iwismer/rusty-timer/actions/workflows/ui.yml/badge.svg?branch=master)](https://github.com/iwismer/rusty-timer/actions/workflows/ui.yml)
[![Embed UI CI](https://github.com/iwismer/rusty-timer/actions/workflows/embed-ui.yml/badge.svg?branch=master)](https://github.com/iwismer/rusty-timer/actions/workflows/embed-ui.yml)

Rusty Timer forwards IPICO chip-timing reads over the internet so your
timing software doesn't need a direct cable to each reader. Use it when
readers are far from the timing tent, at multi-site events, or as a
remote backup for local reads.

It is compatible with any software that works with IPICO readers (tested
with IPICO Connect).

## How It Works

```
             ┌─── Field ───┐          ┌── Cloud ──┐        ┌── Timing Tent ──┐
IPICO Reader ──TCP──► Forwarder ──WS──► Server ──WS──► Receiver ──TCP──► Timing Software
                                           │
                                      Dashboard (web)
```

The **[Forwarder](services/forwarder/)** runs on a small computer (e.g.
Raspberry Pi) next to each IPICO reader. It journals every read to a
local SQLite database for power-loss safety, then forwards reads to a
central server over WebSocket with at-least-once delivery.

The **[Server](services/server/)** ingests reads from all forwarders,
deduplicates them, stores them in PostgreSQL, and fans them out to
receivers over WebSocket. It serves a web dashboard for monitoring
streams, managing races, and exporting data.

The **[Receiver](services/receiver/)** subscribes to one or more streams
from the server and re-exposes each as a local TCP port — so your
existing timing software sees the data as if the reader were plugged in
directly.

If the internet drops, reads are safe: the forwarder journals locally
and replays everything once the connection is restored.

### Other Components

**[Streamer](services/streamer/)** — Connects to one or more IPICO
readers over TCP and fans out reads to any number of local TCP clients.
Useful as a standalone tool without the remote forwarding stack.

**[Announcer](docs/announcer.md)** — A live public-facing screen served
by the server that shows recent finishers. Configurable via the server
dashboard.

**[Server UI](apps/server-ui/)**, **[Receiver UI](apps/receiver-ui/)**,
**[Forwarder UI](apps/forwarder-ui/)** — Web dashboards for each
service (the forwarder and receiver UIs are embedded in their binaries).
Built with SvelteKit.

## Compatibility

**Readers:** Tested with IPICO Lite readers. Should also work with IPICO
Elite and Super Elite readers. Not compatible with non-IPICO hardware.

**Timing software:** Compatible with any software that accepts IPICO TCP
streams — tested with IPICO Connect.

**Forwarder hardware:** Raspberry Pi 3, 4, or 5 (64-bit OS). Any Linux
SBC with network access and an ARM64 or x86-64 CPU should work.

**Performance:** Supports multiple readers and forwarders simultaneously
with sub-second read forwarding latency.

## Quick Demo

Run the full stack locally with simulated readers — no hardware needed:

**Prerequisites:** [Rust](https://rustup.rs/) 1.93.1 (via `rust-toolchain.toml`), [Docker](https://www.docker.com/), [Node.js](https://nodejs.org/) 24.x, Python 3.11+ with [`uv`](https://docs.astral.sh/uv/), and `tmux`.

**Just want to see the server?** Run it with Docker — no Rust needed:

```bash
docker compose -f deploy/quickstart/docker-compose.yml up -d
# Open http://localhost:8080
```

See [deploy/quickstart/](deploy/quickstart/) for details.

```bash
uv run scripts/dev.py
```

This launches Postgres, the server, an emulator (simulated reader), a
forwarder, and a receiver in tmux panes. The server dashboard is at
`http://localhost:8080`. See [scripts/README.md](scripts/README.md) for
all component URLs and options.

## Deploying for Real

| Component | Guide |
|-----------|-------|
| Quickstart (Docker, no build) | [deploy/quickstart/](deploy/quickstart/) |
| Server (Docker) | [deploy/server/](deploy/server/) |
| Forwarder on Raspberry Pi | [deploy/sbc/](deploy/sbc/) |
| Race-day operations | [docs/runbooks/race-day-operator-guide.md](docs/runbooks/race-day-operator-guide.md) |

Pre-built binaries are available on the [Releases](https://github.com/iwismer/rusty-timer/releases) page.

See the [full documentation index](docs/) for all guides, runbooks, and
reference docs.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for building from source, running
tests, and code quality checks.

## Licence

GPL-3.0 — see [LICENCE.txt](LICENCE.txt)
