# Rusty Timer

[![Rust CI](https://github.com/iwismer/rusty-timer/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/iwismer/rusty-timer/actions/workflows/ci.yml)
[![Server UI CI](https://github.com/iwismer/rusty-timer/actions/workflows/server-ui.yml/badge.svg?branch=master)](https://github.com/iwismer/rusty-timer/actions/workflows/server-ui.yml)

Timing utilities for the IPICO chip timing system, with a full remote forwarding suite for distributing race reads across a network.

## Architecture

```
IPICO Reader ──TCP──► Streamer ──fanout──► Local Clients
                          │
                          └── Forwarder ──WS──► Server ──WS──► Receiver
                                                     │
                                               Dashboard (web)
```

## Services

**[Streamer](services/streamer/)** — Connects to one or more IPICO readers over TCP and fans out reads to any number of local TCP clients. Supports file-based backup of all reads.

**[Emulator](services/emulator/)** — Generates synthetic IPICO reads for testing. Can emit reads at a fixed interval or replay from a pre-recorded file.

**[Forwarder](services/forwarder/)** — Connects to IPICO hardware, journals reads to a local SQLite database with power-loss safety, and forwards them to a central server over WebSocket with at-least-once delivery. Includes an embedded web UI for status monitoring.

**[Server](services/server/)** — Central hub that ingests reads from forwarders, deduplicates, stores in PostgreSQL, and fans out to receivers over WebSocket. Serves a web dashboard and REST API for stream management, exports, and administration.

**[Receiver](services/receiver/)** — Subscribes to streams from the server and re-exposes them as local TCP ports, allowing existing race-management software to consume remote timing data as if it were local. Includes an embedded web UI.

## Frontend Apps

**[Server UI](apps/server-ui/)** — SvelteKit web dashboard served by the server. Displays live streams, metrics, and provides export and administration controls.

**[Receiver UI](apps/receiver-ui/)** — SvelteKit web UI embedded in the receiver binary. Manages server connection, stream subscriptions, and displays status.

**[Forwarder UI](apps/forwarder-ui/)** — SvelteKit web UI embedded in the forwarder binary. Shows reader connection status, uplink state, and recent log entries.

**[Shared UI](apps/shared-ui/)** — Shared Svelte component library (`@rusty-timer/shared-ui`) used by all three frontend apps. Provides common components like `LogViewer`, `StatusBadge`, `NavBar`, `Card`, and `DataTable`.

## Shared Libraries

**[ipico-core](crates/ipico-core/)** — Core IPICO chip-read parsing and validation. Parses raw hex frames into structured `ChipRead` values with timestamp, tag ID, and read type.

**[rt-protocol](crates/rt-protocol/)** — WebSocket message protocol definitions for the v1 protocol. Defines all message types exchanged between forwarders, server, and receivers.

**[timer-core](crates/timer-core/)** — Core timing models and TCP worker types for directly connecting to IPICO hardware. Used by the streamer and emulator.

**[rt-updater](crates/rt-updater/)** — Self-update checker and downloader. Checks GitHub Releases for new versions, downloads and verifies archives by SHA-256, and stages binaries for replacement.

**[emulator](crates/emulator/)** — Emulator library (package `rt-emulator`) for read generation, deterministic scenario playback, and fault injection.

**[rt-test-utils](crates/rt-test-utils/)** — Mock WebSocket client and server for integration testing. Provides `MockWsServer` and `MockWsClient` for testing forwarder, server, and receiver interactions.

## Quick Start

**Prerequisites:**
- Rust 1.89.0 (see `rust-toolchain.toml`)
- Docker (for server / integration tests)
- Node.js 24.x and npm 11.x (see `.nvmrc`)

```bash
# Build all services
cargo build --release --workspace

# Run unit tests
cargo test --workspace --lib
```

See [docs/local-testing.md](docs/local-testing.md) for full local development setup and [deploy/server/](deploy/server/) for production server deployment.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for test commands, code quality checks, and git hook setup.

## Licence

GPL3 — see LICENCE.txt
