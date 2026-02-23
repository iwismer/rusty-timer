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
- `apps/server-ui/` — SvelteKit static web dashboard (served by the server)
- `apps/receiver-ui/` — SvelteKit static frontend for the receiver (embedded in binary via `--features embed-ui`)
- `crates/rt-protocol/` — Frozen WebSocket message types (WsMessage enum)
- `crates/ipico-core/` — Frozen IPICO chip read parser
- `crates/emulator/` — Emulator library: read generation, scenarios, fault injection
- `crates/rt-test-utils/` — MockWsServer + MockWsClient test helpers

### Key Decisions
- Rust MSRV: 1.85.0; pinned toolchain: 1.93.1 (see `rust-toolchain.toml`)
- Node 24.x / npm 11.x (see root `package.json` + `.nvmrc`)
- Server config: env vars only (`DATABASE_URL`, `BIND_ADDR`, `LOG_LEVEL`)
- Forwarder config: TOML only (no env var overrides)
- sqlx 0.8 offline cache at `services/server/.sqlx/`
- Event delivery: at-least-once; deduplicated by `(forwarder_id, reader_ip, stream_epoch, seq)`

## Git Hooks Setup (run once per clone)

```bash
git config core.hooksPath .githooks
```

The pre-commit hook automatically:
1. Strips registry URL `"resolved"` fields from all `package-lock.json` files (root and `apps/*/`), while keeping local workspace `"resolved"` paths
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
cd apps/server-ui && npm test

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
cd apps/server-ui && npm run format
cd apps/forwarder-ui && npm run format
cd apps/receiver-ui && npm run format
```

## Important Notes

- Integration tests require Docker (for Postgres via testcontainers-rs)
- Never commit without running `bash .githooks/pre-commit` first
- The `.sqlx/` offline cache is at `services/server/.sqlx/` — regenerate with `cargo sqlx prepare` if schema changes
- `docs/plans/` is gitignored; all other docs (runbooks, specs, guides) are tracked
- Clippy is configured with `pedantic = warn` at the workspace level (see `Cargo.toml` `[workspace.lints.clippy]`)
- **Never commit `package-lock.json` files with registry URL `"resolved"` fields** — they leak internal registry URLs and bloat diffs. Keep local workspace path `"resolved"` fields (for workspace links). The pre-commit hook handles this automatically, but if you bypass hooks, clean manually with: `jq 'walk(if type == "object" then with_entries(select(.key != "resolved" or (.value | type) != "string" or (.value | test("^https?://") | not))) else . end)' package-lock.json > /tmp/clean.json && mv /tmp/clean.json package-lock.json`


