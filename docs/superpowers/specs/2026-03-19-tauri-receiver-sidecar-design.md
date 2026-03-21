# Tauri Receiver Sidecar — Design Spec

## Goal

Wrap the existing receiver binary in a Tauri v2 desktop app (Windows-only) using the sidecar pattern. The receiver binary and SvelteKit UI are unchanged. The Tauri shell spawns the receiver, opens a native window, and provides installer + auto-update capabilities.

## Architecture

```
┌─────────────────────────────────────┐
│ Tauri Shell (apps/receiver-ui/      │
│              src-tauri/)             │
│                                     │
│  ┌───────────┐   ┌───────────────┐  │
│  │ WebView2  │──▶│ localhost:9090 │  │
│  │ (SvelteKit│   │ (receiver API) │  │
│  │  SPA)     │   └───────┬───────┘  │
│  └───────────┘           │          │
│                   ┌──────┴───────┐  │
│                   │ Receiver     │  │
│                   │ (sidecar)    │  │
│                   └──────────────┘  │
└─────────────────────────────────────┘
```

The Tauri app spawns the receiver binary as a sidecar with `--no-open-browser`. The webview loads `http://127.0.0.1:9090`. The SvelteKit SPA uses its existing relative `/api/v1/...` fetch paths, which resolve to the sidecar's Axum server. On app exit, the sidecar process is killed.

## Components

### New: `apps/receiver-ui/src-tauri/`

**`Cargo.toml`**
- Dependencies: `tauri`, `tauri-plugin-shell`, `tauri-plugin-updater`, `reqwest` (for health check), `tokio` (for async startup)
- Workspace member added to root `Cargo.toml`

**`src/main.rs`** (~100-150 lines)
- Spawn sidecar via `tauri_plugin_shell` with args `["--no-open-browser"]`
- Health-check loop: poll `http://127.0.0.1:9090/api/v1/version` with 200ms intervals, timeout after 10s
- On health check success: create the main window programmatically via `WebviewWindowBuilder::new()` pointing to `http://127.0.0.1:9090`
- On sidecar exit (unexpected): show error dialog, optionally retry (simple retry loop, max 3 attempts)
- On app close: kill sidecar process via the `CommandChild` handle
- Register Tauri updater plugin for auto-update checks
- On health check failure after timeout: show error dialog explaining the receiver failed to start, with details (e.g., port 9090 may be in use)

**`tauri.conf.json`**

No windows defined in config — the window is created programmatically after the sidecar health check passes. This avoids a race condition where the WebView tries to load before the receiver is ready.

```json
{
  "productName": "Rusty Timer Receiver",
  "identifier": "com.rusty-timer.receiver",
  "version": "1.0.0",
  "bundle": {
    "active": true,
    "targets": ["nsis"],
    "externalBin": ["binaries/receiver"],
    "icon": ["icons/icon.ico"],
    "windows": {
      "webviewInstallMode": { "type": "downloadBootstrapper" }
    }
  },
  "app": {
    "windows": []
  },
  "plugins": {
    "updater": {
      "pubkey": "<generated-public-key>",
      "endpoints": [
        "https://github.com/iwismer/rusty-timer/releases/download/receiver-ui-latest/update-manifest.json"
      ]
    }
  }
}
```

Note: The `version` field tracks the Tauri bundle version, independent of the receiver's `Cargo.toml` version. It is bumped manually (or by a script) when creating a new `receiver-ui-v*` tag and must match the tag version.

**`capabilities/default.json`**
```json
{
  "permissions": [
    "core:default",
    {
      "identifier": "shell:allow-execute",
      "allow": [{ "name": "binaries/receiver", "sidecar": true }]
    },
    "shell:allow-kill",
    "updater:default"
  ]
}
```

Note: `shell:allow-execute` is scoped to only the receiver sidecar. `shell:allow-kill` is required to terminate the sidecar on app exit. No broader `shell:allow-spawn` or `shell:allow-stdin-write` needed — the receiver doesn't read from stdin.

**`binaries/`**
- Gitignored (add `apps/receiver-ui/src-tauri/binaries/` to `.gitignore`)
- CI copies the pre-built receiver binary here with the target-triple suffix: `receiver-x86_64-pc-windows-msvc.exe`
- For local dev: `dev.py --tauri` or a manual `cp` copies the debug receiver binary here

### Modified: Root `Cargo.toml`

Add `apps/receiver-ui/src-tauri` to workspace members.

### Modified: `scripts/dev.py`

Add a `--tauri` flag to launch the receiver via Tauri instead of standalone.

**Without `--tauri` (default, unchanged behavior):**
```
RUST_LOG=info ./target/debug/receiver --no-open-browser --receiver-id recv-dev
```

**With `--tauri`:**
1. Build step: `cargo build -p receiver` (without `--features embed-ui` — in Tauri dev mode the SvelteKit frontend is served by Vite's dev server, not the embedded assets, so `embed-ui` is not needed). Also builds the Tauri shell crate.
2. Copy `target/debug/receiver[.exe]` to `apps/receiver-ui/src-tauri/binaries/receiver-{target-triple}[.exe]` (target triple from `rustc --print host-tuple`)
3. Launch pane runs `cargo tauri dev` from `apps/receiver-ui/` instead of the raw receiver binary
4. The receiver auto-config thread works the same way (polls `127.0.0.1:9090`)

Note on hot-reload: `cargo tauri dev` hot-reloads the Tauri Rust shell code. The SvelteKit frontend is served by the receiver binary (via `embed-ui` in release, or via the Vite dev proxy in dev). For frontend hot-reload during Tauri dev, the webview should point to Vite's dev server port (typically 5173) instead of 9090, and the Vite proxy forwards API calls to 9090 as it does today. This is handled by Tauri's `devUrl` config in `tauri.conf.json`.

The `--tauri` flag is informational in the build step — if `--no-build` is passed, the user is responsible for having built both binaries.

### Receiver Tauri jobs in `.github/workflows/release.yml`

Triggered by tags matching `receiver-ui-v*` (same workflow file as other service releases; a `route` job selects Tauri vs standard binary builds).

```yaml
name: Release Tauri Receiver
on:
  push:
    tags: ['receiver-ui-v*']

permissions:
  contents: write

jobs:
  build:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-pc-windows-msvc

      - uses: actions/setup-node@v4
        with:
          node-version: 24

      - name: Install dependencies
        run: npm ci

      # Build the SvelteKit frontend (required for embed-ui)
      - name: Build receiver UI
        run: npm run build --workspace "apps/receiver-ui"

      # Build receiver binary with embedded UI
      # embed-ui is needed because the sidecar serves the SPA over HTTP
      # to the Tauri WebView — the WebView loads http://127.0.0.1:9090
      # which must return the built SvelteKit app
      - name: Build receiver binary
        run: cargo build --release --target x86_64-pc-windows-msvc -p receiver --features receiver/embed-ui

      # Copy sidecar binary with target-triple suffix
      - name: Stage sidecar binary
        shell: bash
        run: |
          mkdir -p apps/receiver-ui/src-tauri/binaries
          cp target/x86_64-pc-windows-msvc/release/receiver.exe apps/receiver-ui/src-tauri/binaries/receiver-x86_64-pc-windows-msvc.exe

      # Install Tauri CLI via cargo-binstall for speed
      - name: Install Tauri CLI
        run: cargo install tauri-cli --version ^2

      # Build Tauri app (produces NSIS installer + .sig file)
      - name: Build Tauri app
        run: cargo tauri build --target x86_64-pc-windows-msvc
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}

      # Generate update manifest JSON for the Tauri updater
      - name: Generate update manifest
        shell: bash
        run: python scripts/generate-tauri-update-manifest.py

      # Upload installer + signature + manifest to GitHub Release
      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          files: |
            target/x86_64-pc-windows-msvc/release/bundle/nsis/*.exe
            target/x86_64-pc-windows-msvc/release/bundle/nsis/*.exe.sig
            update-manifest.json

      # Also upload the manifest to a pinned "receiver-ui-latest" release
      # so the updater endpoint is stable across versions
      - name: Update pinned latest release
        shell: bash
        run: |
          gh release upload receiver-ui-latest update-manifest.json --clobber 2>/dev/null || \
          gh release create receiver-ui-latest update-manifest.json --title "Receiver UI Latest" --notes "Auto-updated manifest for Tauri updater. Do not delete." --latest=false
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

### New: `scripts/generate-tauri-update-manifest.py`

Small script (~40 lines) that:
1. Reads the version from the git tag (`receiver-ui-vX.Y.Z`)
2. Reads the `.sig` file content from the NSIS output directory
3. Writes `update-manifest.json` with all required fields:
```json
{
  "version": "X.Y.Z",
  "pub_date": "2026-03-19T00:00:00Z",
  "notes": "Receiver UI vX.Y.Z",
  "platforms": {
    "windows-x86_64": {
      "url": "https://github.com/iwismer/rusty-timer/releases/download/receiver-ui-vX.Y.Z/Rusty-Timer-Receiver_X.Y.Z_x64-setup.exe",
      "signature": "<contents of .sig file>"
    }
  }
}
```

The `pub_date` is set to the current UTC time. The `notes` field contains a brief version string (can be expanded later to include changelog entries).

### Update Manifest Hosting Strategy

The Tauri updater endpoint points to a **pinned GitHub release** named `receiver-ui-latest` (not GitHub's "latest" release concept). Each Tauri release uploads the new `update-manifest.json` to this pinned release using `gh release upload --clobber`. This avoids the problem where pushing a non-receiver tag (e.g., `server-v0.8.0`) would change GitHub's "latest" release pointer.

The pinned release is created once (by the first Tauri release workflow run) and reused forever. It is marked `--latest=false` so it does not interfere with GitHub's latest release semantics for other services.

### Modified: Existing `release.yml`

No changes. The standalone `receiver-v*` tag flow continues to produce the raw binary archives for headless/Linux use.

### Signing Key Setup (One-Time)

1. `cargo install tauri-cli`
2. `cargo tauri signer generate -w ~/.tauri/rusty-timer-receiver.key`
3. Copy the private key content into GitHub secret `TAURI_SIGNING_PRIVATE_KEY`
4. Copy the password into GitHub secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
5. Copy the public key into `tauri.conf.json` `plugins.updater.pubkey`

The private key never needs to be on any dev machine after this. All releases go through CI.

## Documentation Changes

### Modified: `docs/local-testing.md`

Add a section for running the receiver via Tauri during local development:

**Standalone (existing, unchanged):**
```bash
cargo build -p receiver --features embed-ui
./target/debug/receiver --no-open-browser
# Open http://127.0.0.1:9090 in browser
```

**Via Tauri (new):**
```bash
# Build the receiver binary first
cargo build -p receiver

# Copy to sidecar location (the target triple is for your dev machine)
cp target/debug/receiver apps/receiver-ui/src-tauri/binaries/receiver-$(rustc --print host-tuple)

# Run Tauri dev mode (hot-reloads Tauri Rust shell; SvelteKit served via Vite dev server)
cd apps/receiver-ui && cargo tauri dev
```

### Modified: `scripts/README.md`

Document the new `--tauri` flag:

```
--tauri          Launch receiver via Tauri desktop app instead of standalone binary.
                 Requires `cargo install tauri-cli`. In Tauri dev mode, the SvelteKit
                 frontend is served by Vite (with hot-reload) and the receiver runs
                 as a sidecar process. The receiver auto-config works the same way.
```

### Modified: `docs/receiver-quickstart.md`

Update the "Download" section to reference the Tauri installer as the primary Windows distribution, with the standalone binary as an alternative for advanced users.

### New: `docs/receiver-tauri-dev.md`

Short guide (~50 lines) covering:
1. Prerequisites: Rust, Node 24, `cargo install tauri-cli`, WebView2 (pre-installed on Windows 10 20H2+ and Windows 11)
2. Building and running in dev mode (standalone vs. Tauri)
3. Building a release installer locally
4. How the sidecar pattern works (brief architectural note: Tauri shell spawns receiver binary, WebView loads localhost:9090, SPA served by receiver's embedded assets)
5. Troubleshooting: port 9090 conflicts, WebView2 missing, sidecar not starting

### Modified: `.gitignore`

Add `apps/receiver-ui/src-tauri/binaries/` to prevent accidentally committing sidecar binaries.

## What Does NOT Change

- `services/receiver/` — zero code changes
- `apps/receiver-ui/` SvelteKit code — zero changes
- `crates/rt-updater/` — zero changes (forwarder + standalone receiver keep using it)
- `.github/workflows/release.yml` — extended with Receiver Tauri jobs (Windows NSIS) on `receiver-ui-v*` tags
- `.github/workflows/ci.yml` — unchanged (Tauri release build is only in `release.yml`, not CI)
- Forwarder, server, emulator — unaffected

## Risks and Mitigations

| Risk | Impact | Mitigation |
|------|--------|------------|
| Port 9090 conflict | Sidecar fails to start | Health-check timeout triggers error dialog explaining the issue; sidecar exit code is logged |
| WebView2 missing (old Win10) | App won't launch | NSIS installer downloads bootstrapper automatically |
| Sidecar crashes | Blank window | Detect exit via `CommandChild`, show restart prompt, auto-retry up to 3 times |
| SmartScreen warning | User friction on first install | Accept for now; EV code signing cert is a later investment |
| ~~Two release tags~~ | ~~Operational overhead~~ | Unified: only `receiver-ui-v*` (see `scripts/release.py`) |
| `cargo tauri build` is slow | CI time | Acceptable — runs only on tagged releases, not PRs |
| Duplicate receiver instance | Tauri sidecar + standalone running simultaneously | Tauri shell could check if port 9090 is already occupied before spawning sidecar; if occupied, show "already running" dialog |

## Estimated Effort

| Task | Estimate |
|------|----------|
| Tauri scaffold (`src-tauri/`) | 2-3 hours |
| Sidecar lifecycle in `main.rs` | 2-3 hours |
| Receiver Tauri jobs in `release.yml` | 2-3 hours |
| `generate-tauri-update-manifest.py` | 1 hour |
| `dev.py` `--tauri` flag | 1-2 hours |
| Documentation updates | 1-2 hours |
| Signing key setup + first test build | 1 hour |
| End-to-end testing on Windows | 2-3 hours |
| **Total** | **~1.5-2 days** |
