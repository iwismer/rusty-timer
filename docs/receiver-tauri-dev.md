# Receiver Tauri Development Guide

The receiver can run as a standalone binary (opens in browser) or as a Tauri
desktop app (native window). Both modes use the same receiver binary and
SvelteKit UI — Tauri is a thin shell that spawns the receiver as a sidecar.

## Prerequisites

- Rust (see `rust-toolchain.toml`)
- Node 24
- `cargo install tauri-cli` (Tauri CLI v2)
- WebView2 runtime (pre-installed on Windows 10 20H2+, Windows 11, and macOS/Linux
  use their native webview)

## Running in Dev Mode

### Option 1: Standalone (browser)

```bash
cargo build -p receiver --features embed-ui
./target/debug/receiver
# Opens http://127.0.0.1:9090 in your default browser
```

Or with `dev.py`:

```bash
uv run scripts/dev.py
```

### Option 2: Tauri (native window)

```bash
# Build the receiver binary
cargo build -p receiver

# Copy to sidecar location
mkdir -p apps/receiver-ui/src-tauri/binaries
cp target/debug/receiver apps/receiver-ui/src-tauri/binaries/receiver-$(rustc --print host-tuple)

# Run Tauri dev mode
cd apps/receiver-ui && cargo tauri dev
```

Or with `dev.py`:

```bash
uv run scripts/dev.py --tauri
```

In Tauri dev mode, the SvelteKit frontend is served by Vite (with hot-reload)
and API calls are proxied to the receiver on port 9090.

## Building a Release Installer

```bash
# Build the receiver with embedded UI
cargo build --release -p receiver --features receiver/embed-ui

# Copy to sidecar location
mkdir -p apps/receiver-ui/src-tauri/binaries
cp target/release/receiver.exe apps/receiver-ui/src-tauri/binaries/receiver-x86_64-pc-windows-msvc.exe

# Build the NSIS installer
cd apps/receiver-ui && cargo tauri build
```

The installer is at `target/release/bundle/nsis/`.

Note: building the installer requires the Tauri signing key. In CI, this is
provided via `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
secrets. For local builds without signing, set `"active": false` in
`tauri.conf.json`'s `plugins.updater` section temporarily.

## Architecture

```
Tauri Shell (src-tauri/)
  └── spawns receiver binary as sidecar (--no-open-browser)
       └── receiver serves SvelteKit SPA on 127.0.0.1:9090
            └── Tauri WebView loads http://127.0.0.1:9090
```

The Tauri shell:
1. Spawns the receiver binary with `--no-open-browser`
2. Polls `http://127.0.0.1:9090/api/v1/version` until healthy
3. Creates a native window loading `http://127.0.0.1:9090`
4. Kills the receiver when the window is closed

## Troubleshooting

| Problem | Cause | Fix |
|---------|-------|-----|
| "Failed to spawn receiver" | Sidecar binary missing | Run `cargo build -p receiver` and copy to `src-tauri/binaries/` |
| "Port 9090 may be in use" | Another receiver instance running | Kill the other process or check `lsof -i :9090` |
| Blank window | Receiver crashed after health check | Check terminal output for `[receiver]` log lines |
| WebView2 error on Windows | Runtime not installed | Download from https://developer.microsoft.com/en-us/microsoft-edge/webview2/ |
