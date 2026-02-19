# AGENTS.md — Instructions for AI coding agents

## Agent Notes

- Use `uv` to run Python commands in this workspace.
- Examples:
  - `uv run scripts/dev.py --clear`
  - `uv run --with rich --with iterm2 python -m unittest scripts/tests/test_dev.py`

## Repository Overview

This is the **Rusty Timer Remote Forwarding Suite**, a multi-service Rust workspace with two SvelteKit frontend apps.

### Components
- `services/streamer/` — Connects to IPICO readers, fans out TCP to local clients
- `services/emulator/` — Simulates IPICO reads for development/testing
- `services/forwarder/` — Reads from IPICO hardware, journals to SQLite, forwards over WebSocket
- `services/server/` — Axum/Postgres: ingest, dedup, fanout, dashboard API
- `services/receiver/` — Windows app: subscribes to server, proxies streams to local TCP ports
- `apps/dashboard/` — SvelteKit static web dashboard (served by the server)
- `apps/receiver-ui/` — SvelteKit static frontend for the receiver (embedded in binary via `--features embed-ui`)
- `crates/rt-protocol/` — Frozen WebSocket message types (WsMessage enum)
- `crates/ipico-core/` — Frozen IPICO chip read parser
- `crates/emulator-v2/` — Deterministic multi-reader playback for integration testing
- `crates/rt-test-utils/` — MockWsServer + MockWsClient test helpers

### Key Decisions
- Rust 1.89.0 (see `rust-toolchain.toml`)
- Server config: env vars only (`DATABASE_URL`, `BIND_ADDR`, `LOG_LEVEL`)
- Forwarder config: TOML only (no env var overrides)
- sqlx 0.8 offline cache at `services/server/.sqlx/`
- Event delivery: at-least-once; deduplicated by `(forwarder_id, reader_ip, stream_epoch, seq)`

## Git Hooks Setup (run once per clone)

```bash
git config core.hooksPath .githooks
```

The pre-commit hook automatically:
1. Strips `"resolved"` fields from `apps/*/package-lock.json`
2. Checks Rust formatting: `cargo fmt --all -- --check`
3. Runs Clippy: `cargo clippy --workspace --all-targets`
4. For touched frontend apps, runs `npm run lint` and `npm run check` (blocking)

To run the pre-commit hook manually before committing:
```bash
bash .githooks/pre-commit
```

## Running Tests

```bash
# All Rust unit tests (no Docker needed)
cargo test --workspace --lib

# All tests including integration (Docker required)
cargo test --workspace -- --test-threads=4

# Dashboard unit tests
cd apps/dashboard && npm test

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
cd apps/dashboard && npm run format
cd apps/receiver-ui && npm run format
```

## Important Notes

- Integration tests require Docker (for Postgres via testcontainers-rs)
- Never commit without running `bash .githooks/pre-commit` first
- The `.sqlx/` offline cache is at `services/server/.sqlx/` — regenerate with `cargo sqlx prepare` if schema changes
- `docs/plans/` is gitignored; all other docs (runbooks, specs, guides) are tracked
- Clippy is configured with `pedantic = warn` at the workspace level (see `Cargo.toml` `[workspace.lints.clippy]`)
