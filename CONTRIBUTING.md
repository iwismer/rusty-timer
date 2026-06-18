# Contributing

## Prerequisites

- Rust MSRV: 1.85.0; pinned toolchain: 1.93.1 (see `rust-toolchain.toml`)
- Node.js 24.x and npm 11.x
- Python 3.11+ with `uv`
- Docker only for optional local tooling that explicitly asks for it; the P2P data-plane tests do not require Docker.

JavaScript toolchain pinning:
- `package.json` pins expected Node/npm via `engines`
- `.nvmrc` is set to `24` for `nvm use`

## Running Tests

```bash
# Rust unit tests
cargo test --workspace --lib

# Workspace tests
cargo test --workspace

# Deterministic loopback P2P stack assertions
uv run scripts/e2e/run_stack.py

# Receiver UI tests
(cd apps/receiver-ui && npm test)

# Forwarder UI tests
(cd apps/forwarder-ui && npm test)

# Packaging validation
(cd "$(git rev-parse --show-toplevel)" && bash scripts/validate-packaging.sh)
```

## Code Quality

```bash
# Format Rust
cargo fmt --all

# Lint Rust
cargo clippy --workspace --all-targets

# Format JS/TS
(cd apps/receiver-ui && npm run format)
(cd apps/forwarder-ui && npm run format)
```

## Git Hooks

Run once per clone:

```bash
git config core.hooksPath .githooks
```

The pre-commit hook checks Rust formatting, runs Clippy, and for touched frontend apps runs `npm run lint` and `npm run check`.
