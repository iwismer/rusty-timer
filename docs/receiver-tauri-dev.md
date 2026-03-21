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

`src-tauri/icons/icon.ico` must be a valid multi-size ICO (an empty or truncated
file makes `cargo tauri build` fail on Windows). To regenerate all bundle
icons from a square master PNG (for example 1024×1024), run from
`apps/receiver-ui/src-tauri`: `cargo tauri icon path/to/icon.png`.

Note: building the installer requires the Tauri signing key. In CI, this is
provided via `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
secrets. For local builds without signing, set `"active": false` in
`tauri.conf.json`'s `plugins.updater` section temporarily.

## Shipping a GitHub release (CI)

The [Release workflow](../.github/workflows/release.yml) includes a **Receiver
Tauri** job that runs on tags matching `receiver-ui-vMAJOR.MINOR.PATCH` (for
example `receiver-ui-v0.1.0`). The value after `receiver-ui-v` must match
`version` in `apps/receiver-ui/src-tauri/tauri.conf.json` and match
`services/receiver/Cargo.toml` (same semver).

### One-time: signing key and repository secrets

1. **Generate a keypair** (only if you do not already have one; keep the private
   key secret):

   ```bash
   cargo install tauri-cli --version "^2"
   mkdir -p ~/.tauri
   cargo tauri signer generate -w ~/.tauri/rusty-timer-receiver.key
   ```

   Prefer setting a password when prompted. For unattended CI only, you can use
   `cargo tauri signer generate --ci -w ~/.tauri/rusty-timer-receiver.key`
   (no password; protect the private key exclusively via GitHub Secrets).

2. **Public key in the repo** — `plugins.updater.pubkey` in
   `apps/receiver-ui/src-tauri/tauri.conf.json` must be the **entire** contents
   of the generated `*.key.pub` file (one base64 line), matching the private key
   you use in CI.

3. **GitHub Actions secrets** (repository → *Settings* → *Secrets and variables*
   → *Actions*):

   - `TAURI_SIGNING_PRIVATE_KEY` — paste the **full** contents of the private
     key file (`*.key`), as a single string.
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — the key password, or leave the
     secret **unset** if the key has no password.

### Cut a release

**Recommended:** use the repo release helper so `services/receiver`, Tauri
`tauri.conf.json`, and both tags stay aligned:

```bash
uv run scripts/release.py receiver --patch   # or --minor / --major / --version X.Y.Z
```

That bumps the receiver crate and `apps/receiver-ui/src-tauri/tauri.conf.json`
to the same version, then creates and pushes `receiver-ui-vX.Y.Z` (single tag;
triggers the Tauri Windows build in [`.github/workflows/release.yml`](../.github/workflows/release.yml)).

**Manual alternative:** bump `version` in `tauri.conf.json` to match the tag you
will use, commit, then:

```bash
git tag receiver-ui-v0.1.0
git push origin receiver-ui-v0.1.0
```

Or run the workflow manually (*Actions* → *Release* → *Run workflow*) and pass
an **existing** `receiver-ui-v*` tag name.

The Tauri workflow uploads the NSIS installer to a GitHub Release for
`receiver-ui-v*` and publishes `update-manifest.json` to the
`receiver-ui-latest` release for the in-app updater endpoint configured in
`tauri.conf.json`.

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
