# Documentation

## Getting Started

- **[P2P local testing guide](local-testing.md)** — Run emulator, forwarder, receiver-headless, and thin-node on loopback.
- **[Receiver quickstart (Windows)](receiver-quickstart.md)** — Configure the receiver UI against forwarder endpoint IDs and stream IDs.
- **[Contributing](../CONTRIBUTING.md)** — Building from source, running tests, code quality.

## Deployment

- **[Forwarder on Raspberry Pi](../deploy/sbc/)** — SD card flashing, cloud-init, and setup script.
- **[Systemd services](../deploy/systemd/)** — Unit files for forwarder services.
- **[Network architecture](network-architecture.md)** — P2P iroh, thin-node HTTP, firewall, and auth layout.

## Operations Runbooks

- **[Race-day operator guide](runbooks/race-day-operator-guide.md)** — Start-to-finish flow for race day.
- **[Thin-node operations](runbooks/thin-node-operations.md)** — Provisioning tokens, allow-list distribution, announcer status, and auth posture.
- **[Forwarder operations](runbooks/forwarder-operations.md)** — TOML config, reader health, journal retention, and P2P endpoint checks.
- **[Receiver operations](runbooks/receiver-operations.md)** — Subscriptions, cursors, local proxy replay, and DBF delivery.
- **[Announcer](announcer.md)** — Live public finisher display from thin-node announcer rows.

## Service Reference

- **[Forwarder](../services/forwarder/)** — IPICO reader ingestion, SQLite journal, status/control HTTP, and P2P iroh endpoint.
- **[Receiver](../services/receiver/)** — Durable received-events store, `receiver-headless`, local TCP proxy, DBF writer, and receiver UI IPC.
- **[Thin-node](../services/thin-node/)** — SQLite registry, receiver allow-list distribution, announcer push, and status board.
- **[Streamer](../services/streamer/)** — TCP fanout utility for IPICO readers.
- **[Emulator](../services/emulator/)** — Synthetic IPICO reader for local and E2E tests.

## Frontend Apps

- **[Receiver UI](../apps/receiver-ui/)** — Tauri v2 + SvelteKit desktop app.
- **[Forwarder UI](../apps/forwarder-ui/)** — SvelteKit web UI for forwarder status/control.
- **[Shared UI](../apps/shared-ui/)** — Shared Svelte components and help metadata.

## Protocol & Internals

- **[rt-p2p-protocol](../crates/rt-p2p-protocol/)** — Protobuf messages, frame codec, and P2P negotiation.
- **[rt-iroh](../crates/rt-iroh/)** — Shared iroh endpoint wrapper.
- **[rt-domain](../crates/rt-domain/)** — Shared domain/control types that are not tied to transport.
- **[ipico-core](../crates/ipico-core/)** — IPICO chip-read parsing.
- **[timer-core](../crates/timer-core/)** — Shared race, participant, and chip model.
- **[emulator](../crates/emulator/)** — IPICO reader emulator library.
- **[rt-test-utils](../crates/rt-test-utils/)** — P2P loopback harness utilities.
- **[rt-updater](../crates/rt-updater/)** — Auto-updater workflow.
- **[IPICO control protocol](ipico-protocol/ipico-control-protocol.md)** — Reader control commands.

## Testing

- `cargo test --workspace --lib` — Rust unit tests.
- `cargo test --workspace -- --test-threads=4` — Rust integration tests.
- `uv run scripts/e2e/run_stack.py` — Deterministic loopback P2P stack.
- `cd apps/receiver-ui && npm test && npm run check` — Receiver UI.
- `bash scripts/validate-packaging.sh` — Release/package metadata checks.
