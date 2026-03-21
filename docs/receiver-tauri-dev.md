# Receiver Tauri Development Guide

The receiver runs as a Tauri desktop app with the receiver library embedded
directly in the Tauri process. The SvelteKit UI communicates with the receiver
via Tauri IPC (invoke/listen), not HTTP.

## Prerequisites

- Rust (see `rust-toolchain.toml`)
- Node 24
- `cargo install tauri-cli` (Tauri CLI v2)
- WebView2 runtime (pre-installed on Windows 10 20H2+, Windows 11, and macOS/Linux
  use their native webview)

## Running in Dev Mode

```bash
cd apps/receiver-ui && cargo tauri dev
```

Or with `dev.py`:

```bash
uv run scripts/dev.py
```

In Tauri dev mode, the SvelteKit frontend is served by Vite (with hot-reload)
and the receiver library runs in-process.

## Building a Release Installer

```bash
# Build the SvelteKit frontend
npm run build --workspace apps/receiver-ui

# Build the NSIS installer
cd apps/receiver-ui && cargo tauri build
```

The installer is at `target/release/bundle/nsis/`.

`src-tauri/icons/icon.ico` must be a valid multi-size ICO (an empty or truncated
file makes `cargo tauri build` fail on Windows). To regenerate all bundle
icons from a square master PNG (for example 1024x1024), run from
`apps/receiver-ui/src-tauri`: `cargo tauri icon path/to/icon.png`.

Note: building the installer requires the Tauri signing key. In CI, this is
provided via `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
secrets. For local builds without signing, set `"active": false` in
`tauri.conf.json`'s `plugins.updater` section temporarily.

## Shipping a GitHub release (CI)

The [Release workflow](../.github/workflows/release.yml) includes a **Receiver
Tauri** job that runs on tags matching `receiver-ui-vMAJOR.MINOR.PATCH` (for
example `receiver-ui-v0.1.0`). The value after `receiver-ui-v` must match
`version` in `apps/receiver-ui/src-tauri/tauri.conf.json`.

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

2. **Public key in the repo** -- `plugins.updater.pubkey` in
   `apps/receiver-ui/src-tauri/tauri.conf.json` must be the **entire** contents
   of the generated `*.key.pub` file (one base64 line), matching the private key
   you use in CI.

3. **GitHub Actions secrets** (repository -> *Settings* -> *Secrets and variables*
   -> *Actions*):

   - `TAURI_SIGNING_PRIVATE_KEY` -- paste the **full** contents of the private
     key file (`*.key`), as a single string.
   - `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` -- the key password, or leave the
     secret **unset** if the key has no password.

### Cut a release

**Recommended:** use the repo release helper so Tauri
`tauri.conf.json` and the tag stay aligned:

```bash
uv run scripts/release.py receiver --patch   # or --minor / --major / --version X.Y.Z
```

That bumps `apps/receiver-ui/src-tauri/tauri.conf.json`
to the target version, then creates and pushes `receiver-ui-vX.Y.Z` (single tag;
triggers the Tauri Windows build in [`.github/workflows/release.yml`](../.github/workflows/release.yml)).

**Manual alternative:** bump `version` in `tauri.conf.json` to match the tag you
will use, commit, then:

```bash
git tag receiver-ui-v0.1.0
git push origin receiver-ui-v0.1.0
```

Or run the workflow manually (*Actions* -> *Release* -> *Run workflow*) and pass
an **existing** `receiver-ui-v*` tag name.

The Tauri workflow uploads the NSIS installer to a GitHub Release for
`receiver-ui-v*` and publishes `update-manifest.json` to the
`receiver-ui-latest` release for the in-app updater endpoint configured in
`tauri.conf.json`.

## Architecture

```
Tauri App (src-tauri/)
  +-- embeds receiver library (services/receiver)
  +-- SvelteKit UI communicates via Tauri IPC (invoke/listen)
```

The Tauri app:
1. Initializes the receiver runtime (opens SQLite DB, restores profile)
2. Spawns the receiver event loop as a tokio task
3. Bridges receiver events to the frontend via Tauri event emitter
4. Exposes receiver API as Tauri IPC commands

## Troubleshooting

| Problem | Cause | Fix |
|---------|-------|-----|
| App fails to start | SQLite DB corruption or missing data directory | Delete the receiver SQLite DB and restart |
| Blank window | Receiver runtime panic or WebView error | Check terminal output for error logs |
| WebView2 error on Windows | Runtime not installed | Download from https://developer.microsoft.com/en-us/microsoft-edge/webview2/ |

### Packaged Windows app (no console)

Release builds use the Windows GUI subsystem, so errors are easy to miss. On failure, the app writes **`%LOCALAPPDATA%\com.rusty-timer.receiver\crash.log`** (same directory as `app_local_data_dir()`). Read that file first.
