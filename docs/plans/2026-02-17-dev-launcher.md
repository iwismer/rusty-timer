# Dev Launcher Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Create a single `uv run scripts/dev.py` command that sets up all prerequisites and launches all 6 local dev services in a 2×3 split-pane grid using either tmux or iTerm2.

**Architecture:** A UV inline-script Python file at `scripts/dev.py` handles two phases: (1) idempotent setup (Postgres container, temp config/token files, DB seed, cargo build, npm install), and (2) launch (detect tmux or iTerm2 and split into a 2×3 grid with one service per pane). The forwarder needs a `--config <path>` CLI flag so its TOML can be written to `/tmp/rusty-timer-dev/` instead of `/etc/rusty-timer/`.

**Tech Stack:** Python 3.11+, uv inline scripts (PEP 723), rich (terminal output), iterm2 Python API (iTerm2 path), tmux (tmux path), Docker (Postgres), Rust/Cargo, Node/npm.

---

## Pane layout

```
┌──────────────────────┬──────────────────────┐
│  [0] Postgres        │  [1] Server          │
│  docker logs -f ...  │  cargo run -p server │
├──────────────────────┼──────────────────────┤
│  [2] Emulator        │  [3] Forwarder       │
│  cargo run -p emu... │  cargo run -p fwd... │
├──────────────────────┼──────────────────────┤
│  [4] Receiver        │  [5] Dashboard       │
│  cargo run -p recv   │  npm run dev         │
└──────────────────────┴──────────────────────┘
```

---

## Task 1: Add `--config` CLI flag to the forwarder

The forwarder currently hardcodes `/etc/rusty-timer/forwarder.toml` with no override mechanism.
Add a public `load_config_from_path(path: &Path)` function and wire a `--config <path>` CLI flag
to it so the dev script can write config to `/tmp/rusty-timer-dev/forwarder.toml`.

**Files:**
- Modify: `services/forwarder/src/config.rs:124-130` (add `load_config_from_path`)
- Modify: `services/forwarder/src/main.rs:20-33` (parse `--config` arg, call new function)
- Test: `services/forwarder/tests/config_load.rs` (add one test for the new function)

---

### Step 1: Add `load_config_from_path` to `config.rs`

Replace the existing `load_config()` function (lines 124–130) with two functions:

```rust
/// Load forwarder config from a custom path.
pub fn load_config_from_path(path: &Path) -> Result<ForwarderConfig, ConfigError> {
    let toml_str = std::fs::read_to_string(path)
        .map_err(|e| ConfigError::Io(format!("reading config file: {}", e)))?;
    load_config_from_str(&toml_str, path)
}

/// Load forwarder config from the default path `/etc/rusty-timer/forwarder.toml`.
pub fn load_config() -> Result<ForwarderConfig, ConfigError> {
    load_config_from_path(Path::new("/etc/rusty-timer/forwarder.toml"))
}
```

---

### Step 2: Write a failing test for `load_config_from_path`

Add this test to `services/forwarder/tests/config_load.rs` (at the bottom, after the existing tests):

```rust
#[test]
fn load_config_from_path_reads_toml_file() {
    let token_file = write_token_file("dev-token");
    let toml = format!(
        r#"
schema_version = 1

[server]
base_url = "ws://127.0.0.1:8080"

[auth]
token_file = "{}"

[[readers]]
target = "127.0.0.1:10001"
"#,
        token_file.path().display()
    );
    let mut config_file = tempfile::NamedTempFile::new().unwrap();
    std::io::Write::write_all(&mut config_file, toml.as_bytes()).unwrap();

    let cfg = forwarder::config::load_config_from_path(config_file.path())
        .expect("should load from arbitrary path");
    assert_eq!(cfg.server.base_url, "ws://127.0.0.1:8080");
    assert_eq!(cfg.token, "dev-token");
    assert_eq!(cfg.readers[0].target, "127.0.0.1:10001");
}
```

### Step 3: Run test to confirm it fails

```bash
cargo test -p forwarder --test config_load load_config_from_path_reads_toml_file
```

Expected: FAIL — `load_config_from_path` does not exist yet.

---

### Step 4: Apply the `config.rs` change

Apply the change from Step 1 now.

### Step 5: Run test to confirm it passes

```bash
cargo test -p forwarder --test config_load load_config_from_path_reads_toml_file
```

Expected: PASS.

Also run all forwarder tests to confirm nothing regressed:

```bash
cargo test -p forwarder
```

Expected: all pass.

---

### Step 6: Update `main.rs` to parse `--config`

Replace the `load_config()` call block in `main.rs` with:

```rust
fn main() {
    // Initialize tracing subscriber for structured logging to stdout.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!(version = env!("CARGO_PKG_VERSION"), "forwarder starting");

    // Parse optional --config <path> argument.
    // Defaults to /etc/rusty-timer/forwarder.toml when not supplied.
    let args: Vec<String> = std::env::args().collect();
    let config_path = match args.iter().position(|a| a == "--config") {
        Some(i) => match args.get(i + 1) {
            Some(p) => std::path::PathBuf::from(p),
            None => {
                eprintln!("FATAL: --config requires a path argument");
                std::process::exit(1);
            }
        },
        None => std::path::PathBuf::from("/etc/rusty-timer/forwarder.toml"),
    };

    let _cfg = match forwarder::config::load_config_from_path(&config_path) {
        Ok(cfg) => {
            info!(
                base_url = %cfg.server.base_url,
                readers = cfg.readers.len(),
                "config loaded"
            );
            cfg
        }
        Err(e) => {
            eprintln!("FATAL: failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    // Task 6+: init SQLite journal, uplink, fanout, status HTTP.
    info!("forwarder initialized (stub — subsystems added in later tasks)");
}
```

### Step 7: Confirm it still compiles and tests pass

```bash
cargo test -p forwarder
```

Expected: all tests pass.

---

### Step 8: Commit

```bash
git add services/forwarder/src/config.rs \
        services/forwarder/src/main.rs \
        services/forwarder/tests/config_load.rs
git commit -m "feat(forwarder): add --config <path> CLI flag for custom config file location"
```

---

## Task 2: Create `scripts/dev.py`

A single Python file using UV inline script metadata (PEP 723). Run with `uv run scripts/dev.py`.

**Files:**
- Create: `scripts/dev.py`

**Prerequisites the script assumes:**
- Docker is installed and running
- `cargo` (Rust) is in `$PATH`
- `npm` is in `$PATH`
- **For iTerm2 path only:** iTerm2 has "Enable Python API" turned on
  (Preferences → General → Magic → Enable Python API)

---

### Step 1: Create `scripts/dev.py`

```python
# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "rich>=13",
#   "iterm2>=2.7",
# ]
# ///
"""
Rusty Timer local dev launcher.

Usage:
    uv run scripts/dev.py [--no-build]

Options:
    --no-build   Skip cargo build (use when binaries are already up-to-date)

Launches all 6 services in a 2x3 split-pane grid:

    ┌──────────────┬──────────────┐
    │   Postgres   │    Server    │
    ├──────────────┼──────────────┤
    │   Emulator   │  Forwarder   │
    ├──────────────┼──────────────┤
    │   Receiver   │  Dashboard   │
    └──────────────┴──────────────┘

Multiplexer detection order:
  1. tmux (if in $PATH)
  2. iTerm2 Python API (if /Applications/iTerm.app exists)
  3. Exit with instructions
"""

import asyncio
import hashlib
import shutil
import subprocess
import sys
import time
from pathlib import Path

from rich.console import Console
from rich.panel import Panel

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

REPO_ROOT = Path(__file__).resolve().parent.parent
DEV_DIR = Path("/tmp/rusty-timer-dev")

POSTGRES_CONTAINER = "rt-postgres"
POSTGRES_USER = "rt"
POSTGRES_PASSWORD = "secret"
POSTGRES_DB = "rusty_timer"
POSTGRES_PORT = 5432
DATABASE_URL = (
    f"postgres://{POSTGRES_USER}:{POSTGRES_PASSWORD}"
    f"@localhost:{POSTGRES_PORT}/{POSTGRES_DB}"
)

FORWARDER_TOKEN = "rusty-dev-forwarder"
RECEIVER_TOKEN = "rusty-dev-receiver"

FORWARDER_CONFIG_PATH = DEV_DIR / "forwarder.toml"
FORWARDER_TOKEN_PATH = DEV_DIR / "forwarder-token.txt"
RECEIVER_TOKEN_PATH = DEV_DIR / "receiver-token.txt"
FORWARDER_JOURNAL_PATH = DEV_DIR / "forwarder.sqlite3"

console = Console()

# ---------------------------------------------------------------------------
# Setup helpers
# ---------------------------------------------------------------------------


def sha256_hex(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


def check_prereqs() -> None:
    required = ["docker", "cargo", "npm"]
    missing = [t for t in required if not shutil.which(t)]
    if missing:
        console.print(f"[red]Missing required tools: {', '.join(missing)}[/red]")
        sys.exit(1)
    console.print("[green]✓[/green] Prerequisites OK")


def start_postgres() -> None:
    result = subprocess.run(
        ["docker", "inspect", "-f", "{{.State.Running}}", POSTGRES_CONTAINER],
        capture_output=True,
        text=True,
    )
    if result.returncode == 0 and result.stdout.strip() == "true":
        console.print("[green]✓[/green] Postgres already running")
        return

    # Remove any stopped container with the same name before starting fresh.
    subprocess.run(["docker", "rm", "-f", POSTGRES_CONTAINER], capture_output=True)

    subprocess.run(
        [
            "docker", "run", "--rm", "-d",
            "--name", POSTGRES_CONTAINER,
            "-e", f"POSTGRES_USER={POSTGRES_USER}",
            "-e", f"POSTGRES_PASSWORD={POSTGRES_PASSWORD}",
            "-e", f"POSTGRES_DB={POSTGRES_DB}",
            "-p", f"{POSTGRES_PORT}:5432",
            "postgres:16",
        ],
        check=True,
    )
    console.print("[green]✓[/green] Postgres container started")


def wait_for_postgres(timeout: int = 30) -> None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        r = subprocess.run(
            ["docker", "exec", POSTGRES_CONTAINER, "pg_isready", "-U", POSTGRES_USER],
            capture_output=True,
        )
        if r.returncode == 0:
            console.print("[green]✓[/green] Postgres ready")
            return
        time.sleep(1)
    console.print("[red]Postgres did not become ready within 30 s[/red]")
    sys.exit(1)


def write_dev_files() -> None:
    DEV_DIR.mkdir(parents=True, exist_ok=True)

    FORWARDER_TOKEN_PATH.write_text(FORWARDER_TOKEN + "\n")
    RECEIVER_TOKEN_PATH.write_text(RECEIVER_TOKEN + "\n")

    config = f"""\
schema_version = 1

[server]
base_url = "ws://127.0.0.1:8080"

[auth]
token_file = "{FORWARDER_TOKEN_PATH}"

[journal]
sqlite_path = "{FORWARDER_JOURNAL_PATH}"
prune_watermark_pct = 80

[status_http]
bind = "0.0.0.0:8081"

[uplink]
batch_mode       = "immediate"
batch_flush_ms   = 100
batch_max_events = 50

[[readers]]
target    = "127.0.0.1:10001"
read_type = "raw"
enabled   = true
"""
    FORWARDER_CONFIG_PATH.write_text(config)
    console.print(f"[green]✓[/green] Dev files written to {DEV_DIR}")


def seed_tokens() -> None:
    """Insert dev tokens into Postgres. Idempotent via ON CONFLICT DO NOTHING."""
    entries = [
        (FORWARDER_TOKEN, "forwarder"),
        (RECEIVER_TOKEN, "receiver"),
    ]
    for token, device_type in entries:
        hex_hash = sha256_hex(token)
        sql = (
            f"INSERT INTO device_tokens (token_hash, device_type, device_id) "
            f"VALUES (decode('{hex_hash}', 'hex'), '{device_type}', 'dev') "
            f"ON CONFLICT (token_hash) DO NOTHING;"
        )
        r = subprocess.run(
            [
                "docker", "exec", POSTGRES_CONTAINER,
                "psql", "-U", POSTGRES_USER, "-d", POSTGRES_DB, "-c", sql,
            ],
            capture_output=True,
            text=True,
        )
        if r.returncode != 0:
            console.print(f"[yellow]Token seed warning ({device_type}): {r.stderr.strip()}[/yellow]")
    console.print("[green]✓[/green] Dev tokens seeded")


def build_rust() -> None:
    console.print("Building Rust binaries (this may take a moment)…")
    subprocess.run(
        [
            "cargo", "build",
            "-p", "server",
            "-p", "forwarder",
            "-p", "receiver",
            "-p", "emulator",
        ],
        check=True,
        cwd=REPO_ROOT,
    )
    console.print("[green]✓[/green] Rust binaries built")


def install_dashboard_deps() -> None:
    dashboard = REPO_ROOT / "apps" / "dashboard"
    if not (dashboard / "node_modules").exists():
        console.print("Installing dashboard npm dependencies…")
        subprocess.run(["npm", "install"], check=True, cwd=dashboard)
    console.print("[green]✓[/green] Dashboard dependencies OK")


def setup(skip_build: bool = False) -> None:
    check_prereqs()
    start_postgres()
    wait_for_postgres()
    write_dev_files()
    seed_tokens()
    if not skip_build:
        build_rust()
    install_dashboard_deps()


# ---------------------------------------------------------------------------
# Pane definitions  (order determines grid position)
# ---------------------------------------------------------------------------
# Position in 2x3 grid (row-major, left-to-right):
#   [0] top-left     [1] top-right
#   [2] mid-left     [3] mid-right
#   [4] bot-left     [5] bot-right

PANES: list[tuple[str, str]] = [
    (
        "Postgres",
        f"docker logs -f {POSTGRES_CONTAINER}",
    ),
    (
        "Server",
        f"DATABASE_URL={DATABASE_URL} BIND_ADDR=0.0.0.0:8080 LOG_LEVEL=debug "
        f"cargo run -p server",
    ),
    (
        "Emulator",
        "cargo run -p emulator -- --port 10001 --delay 2000 --type raw",
    ),
    (
        "Forwarder",
        f"cargo run -p forwarder -- --config {FORWARDER_CONFIG_PATH}",
    ),
    (
        "Receiver",
        "cargo run -p receiver",
    ),
    (
        "Dashboard",
        "cd apps/dashboard && npm run dev",
    ),
]

# ---------------------------------------------------------------------------
# tmux launcher
# ---------------------------------------------------------------------------


def launch_tmux() -> None:
    session = "rusty-dev"
    subprocess.run(["tmux", "kill-session", "-t", session], capture_output=True)
    subprocess.run(["tmux", "new-session", "-d", "-s", session], check=True)

    # Create 5 additional panes (total = 6).
    # After each split, re-apply "tiled" so tmux doesn't run out of room.
    for _ in range(5):
        subprocess.run(["tmux", "split-window", "-t", session], check=True)
        subprocess.run(["tmux", "select-layout", "-t", session, "tiled"], check=True)

    # Final tiled layout gives a balanced 2x3 grid.
    subprocess.run(["tmux", "select-layout", "-t", session, "tiled"], check=True)

    for i, (title, cmd) in enumerate(PANES):
        pane = f"{session}:0.{i}"
        subprocess.run(["tmux", "select-pane", "-t", pane, "-T", title], check=True)
        full_cmd = f"cd {REPO_ROOT} && {cmd}"
        subprocess.run(["tmux", "send-keys", "-t", pane, full_cmd, "Enter"], check=True)

    # Attach to the session (blocks until the user detaches with Ctrl-b d).
    subprocess.run(["tmux", "attach-session", "-t", session])


# ---------------------------------------------------------------------------
# iTerm2 launcher  (uses the iterm2 Python API)
# ---------------------------------------------------------------------------
# Requires: iTerm2 → Preferences → General → Magic → Enable Python API  ✓


def launch_iterm2() -> None:
    asyncio.run(_iterm2_async())


async def _iterm2_async() -> None:
    import iterm2  # noqa: PLC0415 — intentional lazy import

    async with iterm2.Connection.async_create() as connection:
        app = await iterm2.async_get_app(connection)
        window = await app.async_create_window()
        tab = window.current_tab
        s0 = tab.current_session

        # Build 2×3 layout:
        #   s0 (top-left)  | s1 (top-right)
        #   s2 (mid-left)  | s4 (mid-right)
        #   s3 (bot-left)  | s5 (bot-right)
        s1 = await s0.async_split_pane(vertical=True)   # right column
        s2 = await s0.async_split_pane(vertical=False)  # left mid
        s3 = await s2.async_split_pane(vertical=False)  # left bot
        s4 = await s1.async_split_pane(vertical=False)  # right mid
        s5 = await s4.async_split_pane(vertical=False)  # right bot

        sessions = [s0, s1, s2, s3, s4, s5]
        for session, (title, cmd) in zip(sessions, PANES):
            await session.async_set_name(title)
            await session.async_send_text(f"cd {REPO_ROOT} && {cmd}\n")


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def detect_and_launch() -> None:
    if shutil.which("tmux"):
        console.print("[blue]Multiplexer:[/blue] tmux")
        launch_tmux()
    elif Path("/Applications/iTerm.app").exists():
        console.print("[blue]Multiplexer:[/blue] iTerm2")
        console.print(
            "[dim]Note: requires Preferences → General → Magic → Enable Python API[/dim]"
        )
        launch_iterm2()
    else:
        console.print("[red]No multiplexer found.[/red]")
        console.print("Install tmux:  brew install tmux")
        console.print("Or install iTerm2: https://iterm2.com")
        sys.exit(1)


def main() -> None:
    skip_build = "--no-build" in sys.argv

    console.print(
        Panel.fit(
            "[bold cyan]Rusty Timer Dev Launcher[/bold cyan]\n"
            "Setting up local dev environment…",
            border_style="cyan",
        )
    )

    setup(skip_build=skip_build)

    console.print("\n[bold green]Setup complete — launching services…[/bold green]\n")
    detect_and_launch()


if __name__ == "__main__":
    main()
```

---

### Step 2: Smoke-test the script parses without error

```bash
uv run scripts/dev.py --help 2>&1 || true
python3 -c "import ast; ast.parse(open('scripts/dev.py').read()); print('parse OK')"
```

Expected: `parse OK` (the script has no `--help` handler but should not crash on parse).

---

### Step 3: Commit

```bash
git add scripts/dev.py
git commit -m "feat: add scripts/dev.py — one-command local dev launcher (tmux + iTerm2)"
```

---

## Verification

After both commits, do a full manual smoke-test:

```bash
# If tmux is installed:
brew install tmux    # if not already present
uv run scripts/dev.py

# Expected result:
# 1. Rich panel prints "Rusty Timer Dev Launcher"
# 2. Postgres container starts (or reuses existing)
# 3. Forwarder TOML and token files appear in /tmp/rusty-timer-dev/
# 4. Tokens are seeded into Postgres
# 5. Cargo build runs for all 4 binaries
# 6. npm install skipped (already done) or runs
# 7. tmux session "rusty-dev" opens with 6 panes in 2×3 grid
# 8. Each pane starts its service

# Re-run is safe:
uv run scripts/dev.py --no-build   # skips cargo build, reuses container
```

Confirm the forwarder `--config` flag works standalone:

```bash
uv run scripts/dev.py --no-build   # writes /tmp/rusty-timer-dev/forwarder.toml
cargo run -p forwarder -- --config /tmp/rusty-timer-dev/forwarder.toml
# Expected: "config loaded" log line, then the stub message
```
