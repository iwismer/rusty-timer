# Rusty Timer

[![Rust CI](https://github.com/iwismer/rusty-timer/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/iwismer/rusty-timer/actions/workflows/ci.yml)
[![Server UI CI](https://github.com/iwismer/rusty-timer/actions/workflows/server-ui.yml/badge.svg?branch=master)](https://github.com/iwismer/rusty-timer/actions/workflows/server-ui.yml)

Timing utilities for the IPICO chip timing system, extended with a full remote forwarding suite for distributing race reads across a network.

## Architecture

```
IPICO Reader ──TCP──► Streamer ──fanout──► Local Clients
                          │
                          └── Forwarder ──WS──► Server ──WS──► Receiver
                                                     │
                                               Dashboard (web)
```

| Component    | Location              | Description                                                      |
|--------------|-----------------------|------------------------------------------------------------------|
| streamer     | services/streamer/    | Connects to IPICO readers, fans out TCP to local clients         |
| emulator     | services/emulator/    | Simulates IPICO reads for development/testing                    |
| forwarder    | services/forwarder/   | Reads from IPICO hardware, journals to SQLite, forwards over WebSocket |
| server       | services/server/      | Axum/Postgres: ingest, dedup, fanout, dashboard API              |
| receiver     | services/receiver/    | Subscribes to server, proxies streams to local TCP ports         |
| server-ui    | apps/server-ui/       | SvelteKit static web dashboard (served by server)                |
| receiver-ui  | apps/receiver-ui/     | Tauri v2 + SvelteKit desktop app for the receiver                |
| emulator-v2  | crates/emulator-v2/   | Deterministic multi-reader playback for integration testing      |

## Quick Start

**Prerequisites:**
- Rust 1.89.0 (see `rust-toolchain.toml`)
- Docker (for server/integration tests)
- Node.js 20.x and npm 10.x

JavaScript toolchain pinning:
- `package.json` pins the expected Node/npm via `engines` and `packageManager`
- `.nvmrc` is set to `20` for `nvm use`

See [docs/local-testing.md](docs/local-testing.md) for local development setup and [deploy/server/README.md](deploy/server/README.md) for production deployment.

## Read Streamer

Connects to one or more IPICO readers and fans out all reads to any number of local TCP clients. Saves reads to a file for backup.

**Build and run:**

```bash
cargo build --release -p streamer
# or run directly:
cargo run -p streamer -- [OPTIONS] <reader_ip>...
```

```
Usage: streamer [OPTIONS] <reader_ip>...

Arguments:
  <reader_ip>...  The socket address of the reader to connect to. Eg. 192.168.0.52:10000

Options:
  -p, --port <port>         The port of the local machine to bind to [default: 10001]
  -t, --type <read_type>    The type of read the reader is sending [default: raw] (raw, fsls)
  -f, --file <file>         The file to output the reads to
  -b, --bibchip <bibchip>   The bib-chip file
  -P, --ppl <participants>  The .ppl participant file (requires --bibchip)
  -B, --buffer              Buffer the output. Use if high CPU use is encountered
  -h, --help                Print help
  -V, --version             Print version
```

**Examples:**

```bash
# Stream from one reader
cargo run -p streamer -- 10.0.0.51:10000

# Stream from two readers on local port 10003
cargo run -p streamer -- 10.0.0.51:10000 10.0.0.52:10000 -p 10003

# Save reads to file
cargo run -p streamer -- -f reads.txt 10.0.0.51:10000
```

## Read Emulator

Generates valid IPICO reads for testing. Can emit synthetic reads at a fixed interval or replay reads from a file.

**Build and run:**

```bash
cargo build --release -p emulator
# or run directly:
cargo run -p emulator -- [OPTIONS]
```

```
Usage: emulator [OPTIONS]

Options:
  -p, --port <port>       The port of the local machine to listen for connections [default: 10001]
  -f, --file <file>       The file to get the reads from
  -d, --delay <delay>     Delay between reads in ms [default: 1000]
  -t, --type <read_type>  The type of read the reader is sending [default: raw] (raw, fsls)
  -h, --help              Print help
  -V, --version           Print version
```

## Development

**Run tests:**

```bash
# Rust unit tests (no Docker required)
cargo test --workspace --lib

# All tests including integration (Docker required)
cargo test --workspace -- --test-threads=4

# Dashboard unit tests
cd apps/server-ui && npm test

# Packaging validation
bash scripts/validate-packaging.sh
```

**Code quality:**

```bash
# Format Rust
cargo fmt --all

# Lint Rust
cargo clippy --workspace --all-targets

# Format JS/TS
cd apps/server-ui && npm run format
cd apps/receiver-ui && npm run format
```

**Git hooks** (run once per clone):

```bash
git config core.hooksPath .githooks
```

The pre-commit hook checks Rust formatting, runs Clippy, and for touched frontend apps runs:
- `npm run lint`
- `npm run check`

## Licence

GPL3

See LICENCE.txt
