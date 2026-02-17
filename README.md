# Rusty Timer

[![CI](https://github.com/iwismer/rusty-timer/actions/workflows/ci.yml/badge.svg?branch=master)](https://github.com/iwismer/rusty-timer/actions/workflows/ci.yml)

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
| dashboard    | apps/dashboard/       | SvelteKit static web dashboard (served by server)                |
| receiver-ui  | apps/receiver-ui/     | Tauri v2 + SvelteKit desktop app for the receiver                |
| emulator-v2  | crates/emulator-v2/   | Deterministic multi-reader playback for integration testing      |

## Quick Start

**Prerequisites:**
- Rust 1.89.0 (see `rust-toolchain.toml`)
- Docker (for server/integration tests)
- Node.js 20+

See [docs/local-testing.md](docs/local-testing.md) for local development setup and [docs/docker-deployment.md](docs/docker-deployment.md) for production deployment.

## Read Streamer

Connects to one or more IPICO readers and fans out all reads to any number of local TCP clients. Saves reads to a file for backup.

**Build and run:**

```bash
cargo build --release --bin streamer
# or run directly:
cargo run --bin streamer -- [OPTIONS] <reader_ip>...
```

```
USAGE:
    streamer [FLAGS] [OPTIONS] <reader_ip>...

FLAGS:
    -h, --help       Prints help information
    -B, --buffer     Buffer the output. Use if high CPU use is encountered
    -V, --version    Prints version information

OPTIONS:
    -b, --bibchip <bibchip>     The bib-chip file
    -f, --file <file>           The file to output the reads to
    -P, --ppl <participants>    The .ppl participant file (requires --bibchip)
    -p, --port <port>           The local port to bind to [default: 10001]
    -t, --type <read_type>      The read type [default: raw] [possible values: raw, fsls]

ARGS:
    <reader_ip>...    Reader socket address, e.g. 192.168.0.52:10000
```

**Examples:**

```bash
# Stream from one reader (OS-assigned local port)
streamer 10.0.0.51:10000

# Stream from two readers on local port 10003
streamer 10.0.0.51:10000 10.0.0.52:10000 -p 10003

# Save reads to file
streamer -f reads.txt 10.0.0.51:10000
```

## Read Emulator

Generates valid IPICO reads for testing. Can emit synthetic reads at a fixed interval or replay reads from a file.

**Build and run:**

```bash
cargo build --release --bin emulator
# or run directly:
cargo run --bin emulator -- [OPTIONS]
```

```
USAGE:
    emulator [FLAGS] [OPTIONS]

FLAGS:
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -d, --delay <delay>       Delay between reads in ms [default: 1000]
    -f, --file <file>         Reads file to replay
    -p, --port <port>         Local port to listen on [default: 10001]
    -t, --type <read_type>    Read type [default: raw] [possible values: raw, fsls]
```

## Development

**Run tests:**

```bash
# Rust unit tests (no Docker required)
cargo test --workspace --lib

# All tests including integration (Docker required)
cargo test --workspace -- --test-threads=4

# Dashboard unit tests
cd apps/dashboard && npm test

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
cd apps/dashboard && npm run format
cd apps/receiver-ui && npm run format
```

**Git hooks** (run once per clone):

```bash
git config core.hooksPath .githooks
```

The pre-commit hook checks Rust formatting, runs Clippy, and checks JS/TS formatting via Prettier.

## Licence

GPL3

See LICENCE.txt
