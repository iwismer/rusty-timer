# Documentation

## Getting Started

- **[Quick demo](../scripts/README.md)** — Run the full stack locally with `dev.py` (no hardware needed)
- **[Local testing guide](local-testing.md)** — Step-by-step manual setup of each component
- **[Receiver quickstart (Windows)](receiver-quickstart.md)** — Download, configure, and connect the receiver on Windows
- **[Contributing](../CONTRIBUTING.md)** — Building from source, running tests, code quality

## Deployment

- **[Server deployment](../deploy/server/)** — Docker Compose setup, reverse proxy, token provisioning
- **[Forwarder on Raspberry Pi](../deploy/sbc/)** — SD card flashing, cloud-init, setup script
- **[Systemd services](../deploy/systemd/)** — Service unit files for forwarder and receiver

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
- **[Receiver](../services/receiver/)** — Control API, subscription model, port assignment
- **[Streamer](../services/streamer/)** — CLI flags, multi-reader fanout

## Protocol & Internals

- **[rt-protocol](../crates/rt-protocol/)** — WebSocket message definitions (v1 / v1.2)
- **[ipico-core](../crates/ipico-core/)** — IPICO chip-read parsing
- **[IPICO control protocol](ipico-protocol/ipico-control-protocol.md)** — Reader control commands
