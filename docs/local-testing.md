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
4. `services/server` for registry, allow-list distribution, announcer/status
   state, and auth matrix coverage.

The hard assertions check received event counts, receiver cursors, DBF rows,
TCP proxy replay, and server state. UI-agent artifacts are diagnostic only.

## Manual Dev Stack

To bring the whole stack up and interact with it by hand (instead of running
assertions), use:

```bash
uv run scripts/dev_stack.py
```

This starts the emulator, forwarder, and server on loopback (the forwarder
and server serve their embedded web UIs) and launches the desktop receiver
app via `cargo tauri dev`. It follows the **prod-like flow** — no static
allow-list, no preseeded subscription, no hand-fed node ids:

1. The forwarder and receiver self-register with the server (TOFU).
2. Open the server admin UI and **approve both** the forwarder and receiver.
3. The forwarder fetches the receiver allow-list; the receiver discovers the
   approved forwarder.
4. In the receiver app's Streams tab, the discovered stream appears as
   **Available** — click Subscribe and reads start flowing.

The startup banner prints the server UI, admin, and announcer URLs, the
forwarder UI URL, and the expected stream id. Press Ctrl-C (or close the
receiver app) to tear the whole stack down.

Receiver options:

```bash
uv run scripts/dev_stack.py --receiver headless   # no GUI; control API + data plane
uv run scripts/dev_stack.py --receiver none       # set up only; launch a receiver yourself
uv run scripts/dev_stack.py --read-delay-ms 500   # faster emulated reads
uv run scripts/dev_stack.py --data-dir /tmp/rt-dev  # reuse a receiver data dir (keeps approvals)
```

The desktop app reads its server config from the `RT_P2P_*` / `RT_RECEIVER_*`
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
