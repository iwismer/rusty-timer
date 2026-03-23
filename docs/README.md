# Documentation

## Getting Started

- **[Quickstart (Docker, no build)](../deploy/quickstart/)** — Evaluate the server in 5 minutes
- **[Quick demo (full dev stack)](../scripts/README.md)** — Run everything locally with `dev.py` (simulated readers, no hardware)
- **[Receiver quickstart (Windows)](receiver-quickstart.md)** — Download, configure, and connect the receiver on Windows
- **[Local testing guide](local-testing.md)** — Step-by-step manual setup of each component
- **[Contributing](../CONTRIBUTING.md)** — Building from source, running tests, code quality

## Deployment

- **[Server deployment](../deploy/server/)** — Docker Compose setup, reverse proxy, token provisioning
- **[Forwarder on Raspberry Pi](../deploy/sbc/)** — SD card flashing, cloud-init, setup script
- **[Systemd services](../deploy/systemd/)** — Service unit file for the forwarder
- **[Network architecture](network-architecture.md)** — Ports, firewall rules, and production network layout

## Operations Runbooks

- **[Race-day operator guide](runbooks/race-day-operator-guide.md)** — Start-to-finish flow for race day
- **[Server operations](runbooks/server-operations.md)** — Monitoring, recovery, epoch reset, exports
- **[Forwarder operations](runbooks/forwarder-operations.md)** — Configuration, health checks, journal management
- **[Receiver operations](runbooks/receiver-operations.md)** — Subscriptions, mode switching, troubleshooting
- **[Announcer](announcer.md)** — Live public finisher display: setup, configuration, race-day usage

## Reference

- **[File formats (.ppl, .bibchip)](file-formats.md)** — Participant and chip assignment file formats for race data upload

## Service Reference

Each service has its own README with configuration reference and build instructions:

- **[Forwarder](../services/forwarder/)** — TOML configuration, reader targets, batch settings
- **[Server](../services/server/)** — Environment variables, API endpoints, Docker build
- **[Receiver](../services/receiver/)** — Subscription model, port assignment, Tauri IPC commands
- **[Streamer](../services/streamer/)** — CLI flags, multi-reader fanout
- **[Emulator](../services/emulator/)** — Synthetic IPICO reader for local testing

## Frontend Apps

- **[Server UI](../apps/server-ui/)** — SvelteKit dashboard (streams, metrics, announcer)
- **[Receiver UI](../apps/receiver-ui/)** — Tauri v2 + SvelteKit desktop app
- **[Forwarder UI](../apps/forwarder-ui/)** — SvelteKit web UI for forwarder status/control

## Protocol & Internals

- **[rt-protocol](../crates/rt-protocol/)** — WebSocket message definitions (v1 / v1.2)
- **[ipico-core](../crates/ipico-core/)** — IPICO chip-read parsing
- **[timer-core](../crates/timer-core/)** — Shared timing data model (races, participants, chips)
- **[emulator](../crates/emulator/)** — IPICO reader emulator library (fault injection, multi-reader)
- **[rt-test-utils](../crates/rt-test-utils/)** — MockWsServer + MockWsClient test helpers
- **[rt-updater](../crates/rt-updater/)** — Auto-updater workflow
- **[IPICO control protocol](ipico-protocol/ipico-control-protocol.md)** — Reader control commands

## Testing

- **[Integration tests](../tests/integration/)** — E2E, chaos, and durability tests (requires Docker)
