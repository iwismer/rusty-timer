# Contributing

## Prerequisites

- Rust 1.89.0 (see `rust-toolchain.toml`)
- Docker (for server and integration tests)
- Node.js 24.x and npm 11.x

JavaScript toolchain pinning:
- `package.json` pins expected Node/npm via `engines`
- `.nvmrc` is set to `24` for `nvm use`

## Running Tests

```bash
# Rust unit tests (no Docker required)
cargo test --workspace --lib

# All tests including integration (Docker required)
cargo test --workspace -- --test-threads=4

# Dashboard unit tests
(cd apps/server-ui && npm test)

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
(cd apps/server-ui && npm run format)
(cd apps/receiver-ui && npm run format)
(cd apps/forwarder-ui && npm run format)
```

## Git Hooks

Run once per clone:

```bash
git config core.hooksPath .githooks
```

The pre-commit hook checks Rust formatting, runs Clippy, and for touched frontend apps runs `npm run lint` and `npm run check`.
