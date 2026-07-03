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

# Frontend tests for every npm workspace that defines a test script
npm test --workspaces --if-present

# Frontend type checks for every npm workspace that defines a check script
npm run check --workspaces --if-present

# Packaging validation
(cd "$(git rev-parse --show-toplevel)" && bash scripts/validate-packaging.sh)
```

## Code Quality

```bash
# Format Rust
cargo fmt --all

# Lint Rust (matches the pre-commit hook's shipped-crate scope)
cargo clippy --workspace --all-targets --exclude receiver-tauri

# Format JS/TS
npm run format --workspaces --if-present
```

## Git Hooks

Run once per clone:

```bash
git config core.hooksPath .githooks
```

The pre-commit hook checks Rust formatting, runs Clippy with `receiver-tauri` excluded, and for touched forwarder/receiver frontend apps runs `npm run lint` and `npm run check`.
