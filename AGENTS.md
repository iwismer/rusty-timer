# AGENTS.md — Instructions for AI coding agents

## Agent Notes

- Use `uv` to run Python commands in this workspace.
- Examples:
  - `uv run scripts/e2e/run_stack.py --no-build`
  - `uv run python -m unittest scripts/tests/test_cutover_cleanup.py`

## Repository Overview

This is the **Rusty Timer P2P Remote Forwarding Suite**, a multi-service Rust workspace with receiver and forwarder SvelteKit frontends.

### Components
- `services/streamer/` — Connects to IPICO readers and fans out TCP to local clients.
- `services/emulator/` — Simulates IPICO reads for development/testing.
- `services/forwarder/` — Reads from IPICO hardware, journals to SQLite, exposes status/control HTTP, and serves receiver peers over P2P iroh.
- `services/receiver/` — Windows/headless receiver: dials forwarders over P2P, stores durable received events, proxies streams to local TCP ports, and writes DBF rows.
- `services/server/` — SQLite registry, receiver allow-list distribution, announcer push/status board, and HTTP auth boundary.
- `apps/receiver-ui/` — Tauri v2 + SvelteKit frontend for the receiver.
- `apps/forwarder-ui/` — SvelteKit frontend for forwarder status/control.
- `apps/shared-ui/` — Shared frontend components and help metadata.
- `crates/rt-p2p-protocol/` — Protobuf message types, frame codec, and negotiation.
- `crates/rt-iroh/` — Shared iroh endpoint wrapper.
- `crates/rt-domain/` — Shared transport-independent domain/control types.
- `crates/ipico-core/` — Frozen IPICO chip read parser.
- `crates/emulator/` — Emulator library: read generation, scenarios, fault injection.
- `crates/rt-test-utils/` — P2P loopback test helpers.

### Key Decisions
- Rust MSRV: 1.88.0 or newer (the codebase uses let-chains); pinned toolchain: 1.96.1 (see `rust-toolchain.toml`).
- Node 24.x / npm 11.x (see root `package.json` + `.nvmrc`).
- Forwarder config: TOML only (no env var overrides).
- Server config: env vars for process deployment; SQLite for persistence.
- Event delivery: at-least-once; receiver deduplicates durable events by `(stream_id, seq)`.
- Deterministic CI P2P tests use loopback-only iroh with relays disabled, discovery off, seeded keys, and injected addresses.

## Git Hooks Setup (run once per clone)

```bash
git config core.hooksPath .githooks
```

The pre-commit hook automatically:
1. Strips registry URL `"resolved"` fields from all `package-lock.json` files while keeping local workspace `"resolved"` paths.
2. Checks Rust formatting: `cargo fmt --all -- --check`.
3. Runs Clippy: `cargo clippy --workspace --all-targets`.
4. For touched frontend apps, runs `npm run lint` and `npm run check`.

To run the pre-commit hook manually before committing:
```bash
bash .githooks/pre-commit
```

## Running Tests

```bash
# Rust unit tests
cargo test --workspace --lib

# Rust integration tests
cargo test --workspace -- --test-threads=4

# Deterministic loopback P2P E2E stack
uv run scripts/e2e/run_stack.py

# Receiver UI unit/type checks
cd apps/receiver-ui && npm test && npm run check

# Packaging validation
bash scripts/validate-packaging.sh
```

## Code Quality

```bash
# Format Rust
cargo fmt --all

# Lint Rust
cargo clippy --workspace --all-targets

# Format JS/TS
cd apps/forwarder-ui && npm run format
cd apps/receiver-ui && npm run format
```

## Tools

- `scripts/parse_pcap.py` — Parses `.pcapng` capture files and decodes IPICO protocol frames from reassembled TCP streams. Use this with captures in `docs/ipico-protocol/captures/`.
- `tshark` — Use alongside `scripts/parse_pcap.py` for raw packet payloads, TCP stream details, timing, or capture validation.
- `scripts/e2e/run_stack.py` — Boots the loopback emulator → forwarder → receiver-headless → server stack and runs deterministic assertions.

## Important Notes

- The data plane uses SQLite and P2P iroh; local deterministic data-plane tests do not require Docker.
- Never commit without running `bash .githooks/pre-commit` first.
- **Never commit plan files.** `docs/plans/` is gitignored — do not `git add -f` or force-add any files there.
- Clippy is configured with `pedantic = warn` at the workspace level (see `Cargo.toml` `[workspace.lints.clippy]`).
- **Never commit `package-lock.json` files with registry URL `"resolved"` fields** — they leak internal registry URLs and bloat diffs. Keep local workspace path `"resolved"` fields.
