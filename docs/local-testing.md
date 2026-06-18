# Local Testing

## Prerequisites

| Tool | Version | Notes |
|------|---------|-------|
| Rust | 1.93.1 pinned | Installed by `rustup`; see `rust-toolchain.toml` |
| Node.js | 24.x | Needed for UI tests |
| npm | 11.x | See root `package.json` `engines` |
| Python | 3.11+ | Run scripts through `uv run` |

The P2P data-plane tests use SQLite and deterministic local processes. They do
not require a database container.

## Rust Checks

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace --lib
cargo test --workspace
```

## Frontend Checks

```bash
cd apps/forwarder-ui && npm test && npm run check
cd apps/receiver-ui && npm test && npm run check
```

## Deterministic P2P Stack

Run the full loopback stack with simulated reads:

```bash
uv run scripts/e2e/run_stack.py
```

The orchestrator starts:

1. `services/emulator` for simulated IPICO reads.
2. `services/forwarder` with a local SQLite journal and deterministic iroh
   endpoint.
3. `services/receiver` via `receiver-headless`, with durable received events,
   cursors, DBF output, and local TCP replay.
4. `services/thin-node` for registry, allow-list distribution, announcer/status
   state, and auth matrix coverage.

The hard assertions check received event counts, receiver cursors, DBF rows,
TCP proxy replay, and thin-node state. UI-agent artifacts are diagnostic only.

## Manual Dev Stack

To bring the whole stack up and interact with it by hand (instead of running
assertions), use:

```bash
uv run scripts/dev_stack.py
```

This starts the emulator, forwarder, and thin-node on loopback, preseeds the
receiver with one subscription to the emulated stream, and launches the desktop
receiver app via `cargo tauri dev` wired to the local forwarder over iroh. Press
Ctrl-C to tear the whole stack down.

Receiver options:

```bash
uv run scripts/dev_stack.py --receiver headless   # no GUI; control API + data plane
uv run scripts/dev_stack.py --receiver none       # set up only; launch a receiver yourself
uv run scripts/dev_stack.py --read-delay-ms 500   # faster emulated reads
uv run scripts/dev_stack.py --data-dir /tmp/rt-dev  # reuse a receiver data dir
```

The desktop app reads its P2P config from the `RT_P2P_*` / `RT_RECEIVER_*`
environment variables the script sets. Unlike `scripts/e2e/run_stack.py`, this
runner makes no assertions and runs no failure-injection lanes; it is purely for
manual exploration.

## Power-loss and Chaos Lanes

The default run kills both real receiver and forwarder OS processes in separate
lanes and verifies lossless resume without duplicate DBF rows. To run one lane:

```bash
uv run scripts/e2e/run_stack.py --power-loss-target receiver
uv run scripts/e2e/run_stack.py --power-loss-target forwarder
```

Connectivity chaos and NAT/relay validation are heavier lanes. Keep normal CI
loopback-only; run real NAT/cellular or relay checks manually on suitable
hardware.

## Packaging Validation

```bash
bash scripts/validate-packaging.sh
```

This verifies cutover packaging: no legacy central-service paths, no old wire
contract, forwarder Dockerfile health, runbook coverage, and release workflow
routing.
