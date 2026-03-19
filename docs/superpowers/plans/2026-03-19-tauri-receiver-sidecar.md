# Tauri Receiver Sidecar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wrap the existing receiver binary in a Tauri v2 desktop app using the sidecar pattern, with NSIS installer and auto-updater for Windows distribution.

**Architecture:** A thin Tauri shell (`apps/receiver-ui/src-tauri/`) spawns the receiver as a sidecar process, waits for it to be healthy, then opens a WebView2 window pointing to `http://127.0.0.1:9090`. The receiver binary and SvelteKit UI code are unchanged. CI produces an NSIS installer with a Tauri updater manifest hosted on a pinned GitHub release.

**Tech Stack:** Tauri v2, tauri-plugin-shell, tauri-plugin-updater, NSIS installer, GitHub Actions, Python (manifest generator)

**Spec:** `docs/superpowers/specs/2026-03-19-tauri-receiver-sidecar-design.md`

---

## File Structure

### New Files
| File | Responsibility |
|------|---------------|
| `apps/receiver-ui/src-tauri/Cargo.toml` | Tauri shell crate manifest |
| `apps/receiver-ui/src-tauri/src/main.rs` | Sidecar lifecycle: spawn, health check, window, cleanup |
| `apps/receiver-ui/src-tauri/tauri.conf.json` | Tauri app config: bundle, sidecar, updater |
| `apps/receiver-ui/src-tauri/capabilities/default.json` | Shell plugin permissions |
| `apps/receiver-ui/src-tauri/icons/icon.ico` | App icon (placeholder initially) |
| `apps/receiver-ui/src-tauri/build.rs` | Tauri build script (standard boilerplate) |
| `.github/workflows/release-tauri.yml` | CI: build receiver + Tauri, produce NSIS installer |
| `scripts/generate-tauri-update-manifest.py` | Generate Tauri updater JSON manifest from build artifacts |
| `docs/receiver-tauri-dev.md` | Developer guide for Tauri receiver workflow |

### Modified Files
| File | Change |
|------|--------|
| `Cargo.toml` (root) | Add `apps/receiver-ui/src-tauri` to workspace members |
| `scripts/dev.py` | Add `--tauri` flag, sidecar copy logic, `cargo tauri dev` launch |
| `scripts/README.md` | Document `--tauri` flag |
| `docs/local-testing.md` | Add "Via Tauri" section to receiver step |
| `docs/receiver-quickstart.md` | Update download section for Tauri installer |
| `.gitignore` | Add `apps/receiver-ui/src-tauri/binaries/` |
| `.prettierignore` | Add `src-tauri` |

---

## Task 1: Scaffold Tauri Shell Crate

**Files:**
- Create: `apps/receiver-ui/src-tauri/Cargo.toml`
- Create: `apps/receiver-ui/src-tauri/build.rs`
- Create: `apps/receiver-ui/src-tauri/src/main.rs` (minimal placeholder)
- Create: `apps/receiver-ui/src-tauri/tauri.conf.json`
- Create: `apps/receiver-ui/src-tauri/capabilities/default.json`
- Create: `apps/receiver-ui/src-tauri/icons/icon.ico`
- Modify: `Cargo.toml:2-16` (workspace members)
- Modify: `.gitignore`
- Modify: `.prettierignore`

- [ ] **Step 1: Create `apps/receiver-ui/src-tauri/Cargo.toml`**

```toml
[package]
name = "receiver-tauri"
version = "0.1.0"
edition = "2024"
publish = false

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
tauri-plugin-shell = "2"
tauri-plugin-updater = "2"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
tokio = { version = "1", features = ["time", "sync"] }
```

- [ ] **Step 2: Create `apps/receiver-ui/src-tauri/build.rs`**

```rust
fn main() {
    tauri_build::build();
}
```

- [ ] **Step 3: Create minimal `apps/receiver-ui/src-tauri/src/main.rs`**

Start with a minimal Tauri app that just opens an empty window. We'll add sidecar logic in Task 2.

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 4: Create `apps/receiver-ui/src-tauri/tauri.conf.json`**

```json
{
  "productName": "Rusty Timer Receiver",
  "identifier": "com.rusty-timer.receiver",
  "version": "0.1.0",
  "build": {
    "frontendDist": "http://127.0.0.1:9090",
    "devUrl": "http://127.0.0.1:5173"
  },
  "bundle": {
    "active": true,
    "targets": ["nsis"],
    "externalBin": ["binaries/receiver"],
    "icon": ["icons/icon.ico"],
    "windows": {
      "webviewInstallMode": {
        "type": "downloadBootstrapper"
      }
    }
  },
  "app": {
    "windows": []
  },
  "plugins": {
    "updater": {
      "pubkey": "PLACEHOLDER_REPLACE_AFTER_KEY_GENERATION",
      "endpoints": [
        "https://github.com/iwismer/rusty-timer/releases/download/receiver-ui-latest/update-manifest.json"
      ]
    }
  }
}
```

Note: `frontendDist` is set to the receiver's URL for production builds (the WebView loads the receiver's embedded SPA). `devUrl` points to Vite's dev server for `cargo tauri dev` (enables SvelteKit hot-reload). The `windows` array is empty — the window is created programmatically after the health check (Task 2). The `main.rs` uses `cfg!(debug_assertions)` to choose between `devUrl` (debug) and `frontendDist` (release) when creating the window.

- [ ] **Step 5: Create `apps/receiver-ui/src-tauri/capabilities/default.json`**

```json
{
  "identifier": "default",
  "description": "Default capabilities for the receiver Tauri shell",
  "windows": ["*"],
  "permissions": [
    "core:default",
    {
      "identifier": "shell:allow-execute",
      "allow": [
        {
          "name": "binaries/receiver",
          "sidecar": true
        }
      ]
    },
    "shell:allow-kill",
    "updater:default"
  ]
}
```

- [ ] **Step 6: Create placeholder icon**

Use a simple placeholder `.ico` file. For now, copy any 256x256 `.ico` or generate one:

```bash
# If ImageMagick is available:
convert -size 256x256 xc:#2563eb apps/receiver-ui/src-tauri/icons/icon.ico
# Otherwise, create an empty placeholder (Tauri will warn but build):
touch apps/receiver-ui/src-tauri/icons/icon.ico
```

A proper icon can be designed later.

- [ ] **Step 7: Add to workspace members**

In `Cargo.toml` (root), add `"apps/receiver-ui/src-tauri"` to the `members` list. Add it at the end, after `"services/emulator",`:

```toml
    "services/emulator",
    "apps/receiver-ui/src-tauri",
]
```

- [ ] **Step 8: Update `.gitignore`**

Add at the end of `.gitignore`:

```
# Tauri sidecar binaries (copied during build)
apps/receiver-ui/src-tauri/binaries/
```

- [ ] **Step 9: Update `.prettierignore`**

Add `src-tauri` on a new line (after the existing `target/` line):

```
target/
src-tauri
```

This prevents Prettier from trying to format Rust files in the Tauri crate.

- [ ] **Step 10: Verify the scaffold builds**

Run:
```bash
cargo check -p receiver-tauri
```

Expected: compiles successfully (warnings about unused imports are OK at this stage).

- [ ] **Step 11: Commit**

```bash
git add apps/receiver-ui/src-tauri/ Cargo.toml .gitignore .prettierignore
git commit -m "feat(receiver-tauri): scaffold Tauri v2 shell crate

Add the src-tauri/ directory under apps/receiver-ui/ with Cargo.toml,
tauri.conf.json, capabilities, and a minimal main.rs placeholder.
Register as a workspace member."
```

---

## Task 2: Implement Sidecar Lifecycle

**Files:**
- Modify: `apps/receiver-ui/src-tauri/src/main.rs`

This is the core logic: spawn the receiver sidecar, wait for it to be healthy, create the window, handle cleanup. The Tauri runtime is hard to unit test in isolation, so verification is manual: build and run, confirm the window appears with the receiver UI.

- [ ] **Step 1: Write the full `main.rs`**

Replace the placeholder `apps/receiver-ui/src-tauri/src/main.rs` with:

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::{AppHandle, Manager};
use tauri_plugin_shell::ShellExt;

const RECEIVER_URL: &str = "http://127.0.0.1:9090";
const DEV_URL: &str = "http://127.0.0.1:5173";
const HEALTH_URL: &str = "http://127.0.0.1:9090/api/v1/version";
const HEALTH_POLL_INTERVAL_MS: u64 = 200;
const HEALTH_TIMEOUT_MS: u64 = 10_000;
const MAX_RESTART_ATTEMPTS: u32 = 3;

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = run_sidecar_lifecycle(&handle).await {
                    eprintln!("Fatal: failed to start receiver: {e}");
                    // TODO: add tauri-plugin-dialog for a proper error dialog
                    handle.exit(1);
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

async fn run_sidecar_lifecycle(handle: &AppHandle) -> Result<(), String> {
    let mut attempts = 0;

    loop {
        attempts += 1;
        if attempts > MAX_RESTART_ATTEMPTS {
            return Err(format!(
                "Receiver failed to start after {MAX_RESTART_ATTEMPTS} attempts"
            ));
        }

        if attempts > 1 {
            eprintln!("Restarting receiver (attempt {attempts}/{MAX_RESTART_ATTEMPTS})...");
        }

        // Spawn the sidecar
        let (mut rx, child) = handle
            .shell()
            .sidecar("binaries/receiver")
            .map_err(|e| format!("Failed to create sidecar command: {e}"))?
            .args(["--no-open-browser"])
            .spawn()
            .map_err(|e| format!("Failed to spawn receiver: {e}"))?;

        // Monitor sidecar stdout/stderr in background
        tauri::async_runtime::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    tauri_plugin_shell::process::CommandEvent::Stdout(line) => {
                        print!("[receiver] {}", String::from_utf8_lossy(&line));
                    }
                    tauri_plugin_shell::process::CommandEvent::Stderr(line) => {
                        eprint!("[receiver] {}", String::from_utf8_lossy(&line));
                    }
                    _ => {}
                }
            }
        });

        // Wait for the receiver to be healthy
        match wait_for_healthy().await {
            Ok(()) => {}
            Err(e) => {
                let _ = child.kill();
                eprintln!("Health check failed: {e}");
                continue;
            }
        }

        // Create the main window
        // In dev mode, point to Vite dev server for SvelteKit hot-reload.
        // In release mode, point to the receiver's embedded SPA.
        let url = if cfg!(debug_assertions) { DEV_URL } else { RECEIVER_URL };
        let window = tauri::WebviewWindowBuilder::new(
            handle,
            "main",
            tauri::WebviewUrl::External(url.parse().unwrap()),
        )
        .title("Rusty Timer Receiver")
        .inner_size(1200.0, 800.0)
        .build()
        .map_err(|e| format!("Failed to create window: {e}"))?;

        // Wait for the window to be closed
        let (tx, rx_close) = tokio::sync::oneshot::channel::<()>();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::Destroyed = event {
                let _ = tx.send(());
            }
        });

        let _ = rx_close.await;

        // Kill the sidecar on window close
        let _ = child.kill();
        handle.exit(0);
        return Ok(());
    }
}

async fn wait_for_healthy() -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let deadline =
        tokio::time::Instant::now() + std::time::Duration::from_millis(HEALTH_TIMEOUT_MS);

    loop {
        if tokio::time::Instant::now() >= deadline {
            return Err(
                "Receiver did not become healthy within 10 seconds. \
                 Port 9090 may be in use by another process."
                    .to_string(),
            );
        }

        match client.get(HEALTH_URL).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(()),
            _ => {
                tokio::time::sleep(std::time::Duration::from_millis(HEALTH_POLL_INTERVAL_MS)).await;
            }
        }
    }
}
```

**Important notes for the implementer:**
- Error dialogs are deferred (TODO in the code). To add them later, use `tauri-plugin-dialog` (a separate crate in Tauri v2, not built into `tauri::api`).
- The `CommandEvent` variants may differ slightly — check `tauri_plugin_shell::process::CommandEvent` docs. The key variants are `Stdout(Vec<u8>)` and `Stderr(Vec<u8>)`.
- The window close detection approach may need adjustment. An alternative is to use `app.on_window_event()` at the app level or listen for the `tauri::RunEvent::ExitRequested` event in the `.run()` callback.

- [ ] **Step 2: Verify it compiles**

Run:
```bash
cargo check -p receiver-tauri
```

Expected: compiles successfully. Fix any API differences with the actual Tauri v2 crate.

- [ ] **Step 3: Manual smoke test (if on a platform with WebView2 or equivalent)**

```bash
# Build the receiver
cargo build -p receiver

# Copy to sidecar location
mkdir -p apps/receiver-ui/src-tauri/binaries
cp target/debug/receiver apps/receiver-ui/src-tauri/binaries/receiver-$(rustc --print host-tuple)

# Run Tauri dev mode
cd apps/receiver-ui && cargo tauri dev
```

Expected: a native window opens showing the receiver UI at `http://127.0.0.1:9090`. Closing the window kills the receiver process.

**Note:** This only works on Windows (WebView2) or macOS (WebKit) or Linux (WebKitGTK). If developing on macOS, the smoke test should work — WebKit is built-in.

- [ ] **Step 4: Commit**

```bash
git add apps/receiver-ui/src-tauri/src/main.rs
git commit -m "feat(receiver-tauri): implement sidecar lifecycle

Spawn receiver as sidecar with --no-open-browser, poll health endpoint,
create window on success, kill sidecar on window close. Retry up to 3
times on startup failure."
```

---

## Task 3: Update `dev.py` with `--tauri` Flag

**Files:**
- Modify: `scripts/dev.py:679-689` (build_rust)
- Modify: `scripts/dev.py:1134-1164` (parse_args)
- Modify: `scripts/dev.py:194-219` (build_panes)

- [ ] **Step 1: Add `--tauri` argument to `parse_args()`**

In `scripts/dev.py`, in the `parse_args()` function (around line 1134), add after the `--log-level` argument:

```python
    parser.add_argument(
        "--tauri",
        action="store_true",
        help="Launch receiver via Tauri desktop app instead of standalone binary",
    )
```

- [ ] **Step 2: Update `build_rust()` to build the Tauri crate when `--tauri`**

The `build_rust()` function (line 679) currently has signature `def build_rust(skip_build: bool) -> None`. Change it to accept a `tauri` parameter:

```python
def build_rust(skip_build: bool, *, tauri: bool = False) -> None:
```

After the existing `cargo build` command (which builds receiver with `embed-ui`), add:

```python
    if tauri:
        # Copy receiver binary to sidecar location
        target_triple = (
            subprocess.run(
                ["rustc", "--print", "host-tuple"],
                capture_output=True,
                text=True,
                check=True,
            )
            .stdout.strip()
        )
        sidecar_dir = REPO_ROOT / "apps" / "receiver-ui" / "src-tauri" / "binaries"
        sidecar_dir.mkdir(parents=True, exist_ok=True)

        receiver_bin = "receiver.exe" if sys.platform == "win32" else "receiver"
        src = REPO_ROOT / "target" / "debug" / receiver_bin
        suffix = ".exe" if sys.platform == "win32" else ""
        dst = sidecar_dir / f"receiver-{target_triple}{suffix}"

        shutil.copy2(src, dst)
        print(f"  Copied receiver binary to {dst.relative_to(REPO_ROOT)}")
```

Add `import shutil` at the top of the file if not already present.

- [ ] **Step 3: Update `build_panes()` to use Tauri when `--tauri`**

Change the `build_panes()` signature (line 194) to accept a `tauri` parameter:

```python
def build_panes(
    emulators: list[EmulatorSpec], *, log_level: str = "info", tauri: bool = False
) -> list[tuple[str, str]]:
```

Modify only the Receiver tuple within the `panes_after_emulator` list construction (around line 217). The existing code appends the Forwarder pane via `PANES_AFTER_EMULATOR` and then adds the Receiver pane. Replace just the Receiver pane line with a conditional:

```python
    # Existing: PANES_AFTER_EMULATOR contains the Forwarder pane — do NOT change it.
    # Only change the Receiver tuple that follows it.
    if tauri:
        receiver_cmd = f'cd "{REPO_ROOT}/apps/receiver-ui" && cargo tauri dev'
    else:
        receiver_cmd = (
            f'RUST_LOG={shlex.quote(log_level)} ./target/debug/receiver'
            f' --no-open-browser --receiver-id {RECEIVER_DEVICE_ID}'
        )
    # Replace the existing Receiver tuple in panes_after_emulator with this:
    panes_after_emulator = PANES_AFTER_EMULATOR + [("Receiver", receiver_cmd)]
```

Make sure to keep the `shlex.quote(log_level)` call from the original code for the standalone path.

- [ ] **Step 4: Thread the `--tauri` flag through `detect_and_launch()` and `main()`**

In `detect_and_launch()` (line 1096), add `tauri: bool = False` parameter and pass it to `build_panes()`:

```python
def detect_and_launch(
    emulators, *, bibchip_path=None, ppl_path=None, log_level="info", tauri=False
) -> None:
    panes = build_panes(emulators, log_level=log_level, tauri=tauri)
```

In `main()`, pass `args.tauri` to both `build_rust()` and `detect_and_launch()`:

```python
    build_rust(args.no_build, tauri=args.tauri)
    # ...
    detect_and_launch(
        emulators,
        bibchip_path=args.bibchip,
        ppl_path=args.ppl,
        log_level=args.log_level,
        tauri=args.tauri,
    )
```

- [ ] **Step 5: Verify dev.py still works without `--tauri`**

Run:
```bash
uv run scripts/dev.py --no-build --clear
```

Expected: cleans up dev artifacts as usual, no errors about the new flag.

- [ ] **Step 6: Commit**

```bash
git add scripts/dev.py
git commit -m "feat(dev.py): add --tauri flag for Tauri receiver launch

When --tauri is passed, copies the receiver binary to the sidecar
location and launches 'cargo tauri dev' instead of the standalone
receiver binary."
```

---

## Task 4: Create CI Release Workflow

**Files:**
- Create: `.github/workflows/release-tauri.yml`

- [ ] **Step 1: Create `.github/workflows/release-tauri.yml`**

```yaml
name: Release Tauri Receiver

on:
  push:
    tags: ['receiver-ui-v*']
  workflow_dispatch:
    inputs:
      tag:
        description: 'Existing tag to release (e.g. receiver-ui-v0.1.0)'
        required: true

permissions:
  contents: write

jobs:
  validate:
    runs-on: ubuntu-latest
    outputs:
      version: ${{ steps.parse.outputs.version }}
    steps:
      - name: Determine tag
        id: tag
        run: |
          if [ "${{ github.event_name }}" = "workflow_dispatch" ]; then
            echo "tag=${{ github.event.inputs.tag }}" >> "$GITHUB_OUTPUT"
          else
            echo "tag=${GITHUB_REF_NAME}" >> "$GITHUB_OUTPUT"
          fi

      - name: Validate tag format
        run: |
          TAG="${{ steps.tag.outputs.tag }}"
          if [[ ! "$TAG" =~ ^receiver-ui-v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            echo "Error: Tag '$TAG' does not match pattern receiver-ui-vX.Y.Z"
            exit 1
          fi

      - name: Parse version
        id: parse
        run: |
          TAG="${{ steps.tag.outputs.tag }}"
          VERSION="${TAG#receiver-ui-v}"
          echo "version=$VERSION" >> "$GITHUB_OUTPUT"

      - uses: actions/checkout@v4
        with:
          ref: ${{ steps.tag.outputs.tag }}

      - name: Verify tauri.conf.json version matches tag
        run: |
          CONF_VERSION=$(python3 -c "
          import json
          with open('apps/receiver-ui/src-tauri/tauri.conf.json') as f:
              print(json.load(f)['version'])
          ")
          if [ "$CONF_VERSION" != "${{ steps.parse.outputs.version }}" ]; then
            echo "Error: tauri.conf.json version ($CONF_VERSION) != tag version (${{ steps.parse.outputs.version }})"
            exit 1
          fi

  build:
    needs: validate
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
        with:
          ref: receiver-ui-v${{ needs.validate.outputs.version }}

      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: x86_64-pc-windows-msvc

      - uses: actions/setup-node@v4
        with:
          node-version: 24

      - name: Install npm dependencies
        run: npm ci

      - name: Build receiver UI (SvelteKit)
        run: npm run build --workspace "apps/receiver-ui"

      - name: Build receiver binary
        run: cargo build --release --target x86_64-pc-windows-msvc -p receiver --features receiver/embed-ui

      - name: Stage sidecar binary
        shell: bash
        run: |
          mkdir -p apps/receiver-ui/src-tauri/binaries
          cp target/x86_64-pc-windows-msvc/release/receiver.exe \
             apps/receiver-ui/src-tauri/binaries/receiver-x86_64-pc-windows-msvc.exe

      - name: Install Tauri CLI
        run: cargo install tauri-cli --version "^2"

      - name: Build Tauri app
        run: cargo tauri build --target x86_64-pc-windows-msvc
        env:
          TAURI_SIGNING_PRIVATE_KEY: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY }}
          TAURI_SIGNING_PRIVATE_KEY_PASSWORD: ${{ secrets.TAURI_SIGNING_PRIVATE_KEY_PASSWORD }}

      - name: Generate update manifest
        shell: bash
        run: python scripts/generate-tauri-update-manifest.py ${{ needs.validate.outputs.version }}

      - name: Upload artifacts
        uses: actions/upload-artifact@v4
        with:
          name: tauri-installer
          path: |
            target/x86_64-pc-windows-msvc/release/bundle/nsis/*.exe
            target/x86_64-pc-windows-msvc/release/bundle/nsis/*.exe.sig
            update-manifest.json
          retention-days: 1

  release:
    needs: [validate, build]
    runs-on: ubuntu-latest
    steps:
      - name: Download artifacts
        uses: actions/download-artifact@v4
        with:
          name: tauri-installer
          path: artifacts/

      - name: Create GitHub Release
        uses: softprops/action-gh-release@v2
        with:
          tag_name: receiver-ui-v${{ needs.validate.outputs.version }}
          name: "Receiver UI v${{ needs.validate.outputs.version }}"
          files: artifacts/**/*

      - name: Update pinned latest release with manifest
        shell: bash
        run: |
          gh release upload receiver-ui-latest artifacts/update-manifest.json --clobber 2>/dev/null || \
          gh release create receiver-ui-latest artifacts/update-manifest.json \
            --repo "${{ github.repository }}" \
            --title "Receiver UI Latest" \
            --notes "Auto-updated manifest for Tauri updater. Do not delete this release." \
            --latest=false
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/release-tauri.yml
git commit -m "ci: add Tauri receiver release workflow

Triggered by receiver-ui-v* tags. Builds receiver binary with embed-ui,
stages as sidecar, builds NSIS installer via Tauri CLI, generates update
manifest, and uploads to GitHub Releases. Also maintains a pinned
receiver-ui-latest release for the updater endpoint."
```

---

## Task 5: Create Update Manifest Generator Script

**Files:**
- Create: `scripts/generate-tauri-update-manifest.py`

- [ ] **Step 1: Create `scripts/generate-tauri-update-manifest.py`**

```python
#!/usr/bin/env python3
"""Generate the Tauri updater manifest JSON from build artifacts.

Usage: python scripts/generate-tauri-update-manifest.py <version>

Reads the .sig file from the NSIS build output and writes update-manifest.json
to the current directory.
"""

import json
import sys
from datetime import datetime, timezone
from pathlib import Path

REPO = "iwismer/rusty-timer"
NSIS_DIR = Path("target/x86_64-pc-windows-msvc/release/bundle/nsis")


def main() -> None:
    if len(sys.argv) != 2:
        print(f"Usage: {sys.argv[0]} <version>", file=sys.stderr)
        sys.exit(1)

    version = sys.argv[1]

    # Find the .sig file
    sig_files = list(NSIS_DIR.glob("*.exe.sig"))
    if len(sig_files) != 1:
        print(
            f"Expected exactly one .sig file in {NSIS_DIR}, found {len(sig_files)}",
            file=sys.stderr,
        )
        sys.exit(1)

    signature = sig_files[0].read_text().strip()

    # Find the installer .exe (not the .sig)
    exe_files = [f for f in NSIS_DIR.glob("*.exe") if not f.name.endswith(".sig")]
    if len(exe_files) != 1:
        print(
            f"Expected exactly one .exe file in {NSIS_DIR}, found {len(exe_files)}",
            file=sys.stderr,
        )
        sys.exit(1)

    exe_name = exe_files[0].name

    manifest = {
        "version": f"v{version}",
        "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "notes": f"Receiver UI v{version}",
        "platforms": {
            "windows-x86_64": {
                "url": f"https://github.com/{REPO}/releases/download/receiver-ui-v{version}/{exe_name}",
                "signature": signature,
            }
        },
    }

    output = Path("update-manifest.json")
    output.write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"Wrote {output} for v{version}")


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Commit**

```bash
git add scripts/generate-tauri-update-manifest.py
git commit -m "feat: add Tauri update manifest generator script

Reads .sig and .exe from NSIS build output, writes update-manifest.json
for the Tauri updater plugin."
```

---

## Task 6: Documentation Updates

**Files:**
- Create: `docs/receiver-tauri-dev.md`
- Modify: `docs/local-testing.md:237-253`
- Modify: `docs/receiver-quickstart.md:7-22`
- Modify: `scripts/README.md:53-70`

- [ ] **Step 1: Create `docs/receiver-tauri-dev.md`**

```markdown
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
```

- [ ] **Step 2: Update `docs/local-testing.md` receiver section**

After the existing "Step 6: Run the Receiver" section (line 253), add:

```markdown

### Running via Tauri (Native Window)

Instead of opening the receiver UI in a browser, you can run it as a desktop
app via Tauri:

```bash
# Build the receiver
cargo build -p receiver

# Copy to sidecar location
mkdir -p apps/receiver-ui/src-tauri/binaries
cp target/debug/receiver apps/receiver-ui/src-tauri/binaries/receiver-$(rustc --print host-tuple)

# Run Tauri dev mode
cd apps/receiver-ui && cargo tauri dev
```

This opens a native window instead of a browser tab. The receiver API is
identical — all the control API steps below work the same way.

See `docs/receiver-tauri-dev.md` for more details.
```

- [ ] **Step 3: Update `docs/receiver-quickstart.md` download section**

Replace lines 7-22 with:

```markdown
## Download

### Recommended: Desktop App (Windows)

Download the latest `Rusty-Timer-Receiver_*_x64-setup.exe` from the
[Releases](https://github.com/iwismer/rusty-timer/releases) page.

Run the installer. It will install the app and download WebView2 if needed.

Launch "Rusty Timer Receiver" from the Start Menu.

### Alternative: Standalone Binary

Download `receiver-*-x86_64-pc-windows-msvc.zip` from the
[Releases](https://github.com/iwismer/rusty-timer/releases) page.

Extract the archive and double-click `receiver.exe`. The receiver opens a
web UI in your browser at **http://localhost:9090**.
```

- [ ] **Step 4: Update `scripts/README.md` flags section**

After the last documented flag (`--ppl`, around line 58), add:

```markdown
- `--tauri`: launch the receiver via Tauri desktop app instead of the standalone
  binary. Requires `cargo install tauri-cli`. In Tauri dev mode the SvelteKit
  frontend is served by Vite (with hot-reload) and the receiver runs as a sidecar.
```

- [ ] **Step 5: Commit**

```bash
git add docs/receiver-tauri-dev.md docs/local-testing.md docs/receiver-quickstart.md scripts/README.md
git commit -m "docs: add Tauri receiver development guide and update quickstart

Add docs/receiver-tauri-dev.md with prerequisites, dev/release build
instructions, architecture overview, and troubleshooting. Update
local-testing.md with Tauri dev mode option. Update receiver-quickstart.md
to recommend the Tauri installer for Windows users."
```

---

## Task 7: Signing Key Setup and First Build Verification

This task is manual and environment-dependent. It sets up the signing key and verifies the full pipeline.

**Files:**
- Modify: `apps/receiver-ui/src-tauri/tauri.conf.json` (replace pubkey placeholder)

- [ ] **Step 1: Generate the signing keypair**

```bash
cargo install tauri-cli
cargo tauri signer generate -w ~/.tauri/rusty-timer-receiver.key
```

This outputs:
- A private key (printed to stdout and saved to the file)
- A public key (printed to stdout)
- A password (you choose during generation)

Save all three securely.

- [ ] **Step 2: Add secrets to GitHub**

Go to the repo Settings > Secrets and variables > Actions. Add:
- `TAURI_SIGNING_PRIVATE_KEY` — the full private key content
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` — the password

- [ ] **Step 3: Update `tauri.conf.json` with the public key**

Replace the `PLACEHOLDER_REPLACE_AFTER_KEY_GENERATION` value in `apps/receiver-ui/src-tauri/tauri.conf.json` with the actual public key string.

- [ ] **Step 4: Commit**

```bash
git add apps/receiver-ui/src-tauri/tauri.conf.json
git commit -m "chore(receiver-tauri): set updater public key"
```

- [ ] **Step 5: Test the full build locally (Windows)**

```bash
# Build receiver with embedded UI
cargo build --release -p receiver --features receiver/embed-ui

# Copy sidecar
mkdir -p apps/receiver-ui/src-tauri/binaries
cp target/release/receiver.exe apps/receiver-ui/src-tauri/binaries/receiver-x86_64-pc-windows-msvc.exe

# Build Tauri (set signing env vars)
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.tauri/rusty-timer-receiver.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="your-password-here"
cd apps/receiver-ui && cargo tauri build
```

Expected: NSIS installer produced at `target/release/bundle/nsis/`. Install it, verify the app launches and shows the receiver UI.

- [ ] **Step 6: Test the manifest generator**

```bash
python scripts/generate-tauri-update-manifest.py 0.1.0
cat update-manifest.json
```

Expected: valid JSON with `version`, `pub_date`, `platforms.windows-x86_64.url`, and `signature` fields.

- [ ] **Step 7: Tag and push to test CI (optional, only when ready)**

```bash
git tag receiver-ui-v0.1.0
git push origin receiver-ui-v0.1.0
```

Monitor the GitHub Actions workflow. Verify it produces the NSIS installer, .sig, and update-manifest.json on the release page, and creates/updates the `receiver-ui-latest` pinned release.
