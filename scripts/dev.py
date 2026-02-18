# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "rich>=13",
#   "iterm2>=2.7",
# ]
# ///

"""
Rusty Timer Dev Launcher
========================
One-command local dev environment setup and launch.

Usage:
    uv run scripts/dev.py
    uv run scripts/dev.py --no-build
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

REPO_ROOT = Path(__file__).resolve().parent.parent

TMP_DIR = Path("/tmp/rusty-timer-dev")
DEV_DIR = TMP_DIR
FORWARDER_TOML_PATH = TMP_DIR / "forwarder.toml"
FORWARDER_TOKEN_PATH = TMP_DIR / "forwarder-token.txt"
RECEIVER_TOKEN_PATH = TMP_DIR / "receiver-token.txt"
FORWARDER_JOURNAL_PATH = DEV_DIR / "forwarder.sqlite3"

FORWARDER_TOKEN_TEXT = "rusty-dev-forwarder"
RECEIVER_TOKEN_TEXT = "rusty-dev-receiver"

PG_CONTAINER = "rt-postgres"
PG_USER = "rt"
PG_PASSWORD = "secret"
PG_DB = "rusty_timer"
PG_PORT = 5432

PANES = [
    ("Postgres",  f"docker logs -f {PG_CONTAINER}"),
    (
        "Server",
        f"DATABASE_URL=postgres://{PG_USER}:{PG_PASSWORD}@localhost:{PG_PORT}/{PG_DB} "
        f"BIND_ADDR=0.0.0.0:8080 LOG_LEVEL=debug cargo run -p server",
    ),
    ("Emulator",  "cargo run -p emulator -- --port 10001 --delay 2000 --type raw"),
    ("Forwarder", f"cargo run -p forwarder -- --config {FORWARDER_TOML_PATH}"),
    ("Receiver",  "cargo run -p receiver"),
    ("Dashboard", "cd apps/dashboard && npm run dev"),
]

FORWARDER_TOML = f"""\
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

console = Console()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def sha256_hex(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def check_prereqs() -> None:
    console.print("[bold]Checking prerequisites…[/bold]")
    missing = []
    for tool in ("docker", "cargo", "npm"):
        if shutil.which(tool) is None:
            missing.append(tool)
        else:
            console.print(f"  [green]OK[/green]  {tool}")
    if missing:
        console.print(f"[red]Missing tools: {', '.join(missing)}[/red]")
        sys.exit(1)


def start_postgres() -> None:
    console.print("[bold]Starting Postgres container…[/bold]")
    result = subprocess.run(
        ["docker", "inspect", "-f", "{{.State.Running}}", PG_CONTAINER],
        capture_output=True,
        text=True,
    )
    if result.returncode == 0 and result.stdout.strip() == "true":
        console.print(f"  [green]Reusing existing container:[/green] {PG_CONTAINER}")
        return

    # Container exists but is stopped — remove it so we can recreate cleanly
    if result.returncode == 0:
        console.print(f"  [yellow]Removing stopped container:[/yellow] {PG_CONTAINER}")
        subprocess.run(["docker", "rm", PG_CONTAINER], check=True, capture_output=True)

    console.print(f"  [cyan]Creating container:[/cyan] {PG_CONTAINER}")
    subprocess.run(
        [
            "docker", "run", "-d",
            "--name", PG_CONTAINER,
            "-e", f"POSTGRES_USER={PG_USER}",
            "-e", f"POSTGRES_PASSWORD={PG_PASSWORD}",
            "-e", f"POSTGRES_DB={PG_DB}",
            "-p", f"{PG_PORT}:5432",
            "postgres:16",
        ],
        check=True,
    )


def wait_for_postgres() -> None:
    console.print("[bold]Waiting for Postgres to accept connections…[/bold]")
    for attempt in range(30):
        result = subprocess.run(
            [
                "docker", "exec", PG_CONTAINER,
                "pg_isready", "-U", PG_USER, "-d", PG_DB,
            ],
            capture_output=True,
        )
        if result.returncode == 0:
            console.print("  [green]Postgres is ready.[/green]")
            return
        time.sleep(1)
        if attempt % 5 == 4:
            console.print(f"  [dim]Still waiting… ({attempt + 1}s)[/dim]")
    console.print("[red]Postgres did not become ready within 30 seconds.[/red]")
    sys.exit(1)


def write_config_files() -> None:
    console.print("[bold]Writing config files…[/bold]")
    TMP_DIR.mkdir(parents=True, exist_ok=True)
    FORWARDER_TOML_PATH.write_text(FORWARDER_TOML)
    console.print(f"  [green]Wrote[/green] {FORWARDER_TOML_PATH}")
    FORWARDER_TOKEN_PATH.write_text(FORWARDER_TOKEN_TEXT)
    console.print(f"  [green]Wrote[/green] {FORWARDER_TOKEN_PATH}")
    RECEIVER_TOKEN_PATH.write_text(RECEIVER_TOKEN_TEXT)
    console.print(f"  [green]Wrote[/green] {RECEIVER_TOKEN_PATH}")


def seed_tokens() -> None:
    console.print("[bold]Seeding dev tokens into Postgres…[/bold]")
    for token_text, device_type in [
        (FORWARDER_TOKEN_TEXT, "forwarder"),
        (RECEIVER_TOKEN_TEXT, "receiver"),
    ]:
        hex_hash = sha256_hex(token_text)
        # hex_hash is [0-9a-f] only (SHA-256 output); device_type is a hardcoded string literal.
        # Neither comes from external input, so f-string interpolation is safe here.
        sql = (
            f"INSERT INTO device_tokens (token_hash, device_type, device_id) "
            f"VALUES (decode('{hex_hash}', 'hex'), '{device_type}', 'dev') "
            f"ON CONFLICT (token_hash) DO NOTHING;"
        )
        result = subprocess.run(
            [
                "docker", "exec", PG_CONTAINER,
                "psql", "-U", PG_USER, "-d", PG_DB, "-c", sql,
            ],
            capture_output=True,
            text=True,
        )
        if result.returncode != 0:
            console.print(f"[red]Failed to seed token for {device_type}:[/red]\n{result.stderr}")
            sys.exit(1)
        console.print(f"  [green]Seeded[/green] {device_type} token (sha256={hex_hash[:16]}…)")


def build_rust(skip_build: bool) -> None:
    if skip_build:
        console.print("[dim]Skipping Rust build (--no-build)[/dim]")
        return
    console.print("[bold]Building Rust binaries…[/bold]")
    subprocess.run(
        ["cargo", "build", "-p", "server", "-p", "forwarder", "-p", "receiver", "-p", "emulator"],
        check=True,
        cwd=REPO_ROOT,
    )
    console.print("  [green]Build complete.[/green]")


def npm_install() -> None:
    dashboard_dir = REPO_ROOT / "apps" / "dashboard"
    node_modules = dashboard_dir / "node_modules"
    if node_modules.exists():
        console.print("[dim]node_modules present — skipping npm install.[/dim]")
        return
    console.print("[bold]Running npm install in apps/dashboard…[/bold]")
    subprocess.run(["npm", "install"], check=True, cwd=dashboard_dir)
    console.print("  [green]npm install complete.[/green]")


def setup(skip_build: bool = False) -> None:
    check_prereqs()
    start_postgres()
    wait_for_postgres()
    write_config_files()
    seed_tokens()
    build_rust(skip_build=skip_build)
    npm_install()


# ---------------------------------------------------------------------------
# tmux launcher
# ---------------------------------------------------------------------------

def launch_tmux() -> None:
    session = "rusty-dev"
    subprocess.run(["tmux", "kill-session", "-t", session], capture_output=True)
    subprocess.run(["tmux", "new-session", "-d", "-s", session], check=True)
    for _ in range(5):
        subprocess.run(["tmux", "split-window", "-t", session], check=True)
        subprocess.run(["tmux", "select-layout", "-t", session, "tiled"], check=True)
    subprocess.run(["tmux", "select-layout", "-t", session, "tiled"], check=True)
    for i, (title, cmd) in enumerate(PANES):
        pane = f"{session}:0.{i}"
        subprocess.run(["tmux", "select-pane", "-t", pane, "-T", title], check=True)
        full_cmd = f'cd "{REPO_ROOT}" && {cmd}'
        subprocess.run(["tmux", "send-keys", "-t", pane, full_cmd, "Enter"], check=True)
    subprocess.run(["tmux", "attach-session", "-t", session])


# ---------------------------------------------------------------------------
# iTerm2 launcher (iterm2 Python API)
# ---------------------------------------------------------------------------

def launch_iterm2() -> None:
    asyncio.run(_iterm2_async())


async def _iterm2_async() -> None:
    import iterm2  # lazy import — only used on iTerm2 path
    async with iterm2.Connection.async_create() as connection:
        app = await iterm2.async_get_app(connection)
        window = await app.async_create_window()
        tab = window.current_tab
        s0 = tab.current_session
        # Physical layout after splits (row-major):
        #   s0 (top-left)  | s1 (top-right)
        #   s2 (mid-left)  | s4 (mid-right)
        #   s3 (bot-left)  | s5 (bot-right)
        # Row-major sessions list: [s0, s1, s2, s4, s3, s5]
        s1 = await s0.async_split_pane(vertical=True)
        s2 = await s0.async_split_pane(vertical=False)
        s3 = await s2.async_split_pane(vertical=False)
        s4 = await s1.async_split_pane(vertical=False)
        s5 = await s4.async_split_pane(vertical=False)
        sessions = [s0, s1, s2, s4, s3, s5]  # row-major: top-L, top-R, mid-L, mid-R, bot-L, bot-R
        for session, (title, cmd) in zip(sessions, PANES):
            await session.async_set_name(title)
            await session.async_send_text(f'cd "{REPO_ROOT}" && {cmd}\n')


# ---------------------------------------------------------------------------
# Detect and launch
# ---------------------------------------------------------------------------

def detect_and_launch() -> None:
    if shutil.which("tmux"):
        console.print("[blue]Multiplexer:[/blue] tmux")
        launch_tmux()
    elif Path("/Applications/iTerm.app").exists():
        console.print("[blue]Multiplexer:[/blue] iTerm2")
        console.print("[dim]Note: requires Preferences → General → Magic → Enable Python API[/dim]")
        launch_iterm2()
    else:
        console.print("[red]No multiplexer found.[/red]")
        console.print("Install tmux:  brew install tmux")
        console.print("Or install iTerm2: https://iterm2.com")
        sys.exit(1)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def main() -> None:
    skip_build = "--no-build" in sys.argv
    console.print(Panel.fit(
        "[bold cyan]Rusty Timer Dev Launcher[/bold cyan]\n"
        "Setting up local dev environment…",
        border_style="cyan",
    ))
    setup(skip_build=skip_build)
    console.print("\n[bold green]Setup complete — launching services…[/bold green]\n")
    detect_and_launch()


if __name__ == "__main__":
    main()
