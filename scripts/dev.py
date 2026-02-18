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
    uv run scripts/dev.py --clear
    uv run scripts/dev.py --emulator port=10001,delay=500,file=start.txt
    uv run scripts/dev.py --emulator port=10001 --emulator port=10002,delay=500
"""

import argparse
import hashlib
import shlex
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

from rich.console import Console
from rich.panel import Panel

REPO_ROOT = Path(__file__).resolve().parent.parent

TMP_DIR = Path("/tmp/rusty-timer-dev")
FORWARDER_TOML_PATH = TMP_DIR / "forwarder.toml"
FORWARDER_TOKEN_PATH = TMP_DIR / "forwarder-token.txt"
RECEIVER_TOKEN_PATH = TMP_DIR / "receiver-token.txt"
FORWARDER_JOURNAL_PATH = TMP_DIR / "forwarder.sqlite3"

FORWARDER_TOKEN_TEXT = "rusty-dev-forwarder"
RECEIVER_TOKEN_TEXT = "rusty-dev-receiver"

# device_id values must match what each service sends in its hello message:
#   forwarder: "fwd-" + sha256(token_bytes).hex()[:16]  (see services/forwarder/src/main.rs)
#   receiver:  "receiver-main"                          (see services/receiver/src/main.rs)
FORWARDER_DEVICE_ID = "fwd-" + hashlib.sha256(FORWARDER_TOKEN_TEXT.encode()).hexdigest()[:16]
RECEIVER_DEVICE_ID  = "receiver-main"

PG_CONTAINER = "rt-postgres"
PG_USER = "rt"
PG_PASSWORD = "secret"
PG_DB = "rusty_timer"
PG_PORT = 5432

EMULATOR_DEFAULT_DELAY = 2000
EMULATOR_DEFAULT_PORT = 10001
EMULATOR_VALID_TYPES = ("raw", "fsls")


@dataclass
class EmulatorSpec:
    port: int
    delay: int = EMULATOR_DEFAULT_DELAY
    file: str | None = None
    read_type: str = "raw"

    def to_cmd(self) -> str:
        cmd = f"cargo run -p emulator -- --port {self.port} --delay {self.delay} --type {self.read_type}"
        if self.file:
            cmd += f" --file {shlex.quote(self.file)}"
        return cmd

    def to_reader_toml(self) -> str:
        return (
            f"[[readers]]\n"
            f'target              = "127.0.0.1:{self.port}"\n'
            f'read_type           = "{self.read_type}"\n'
            f"enabled             = true\n"
            f"local_fallback_port = {self.port + 1000}\n"
        )


def parse_emulator_spec(value: str) -> EmulatorSpec:
    """Parse 'port=10001,delay=500,file=start.txt,type=raw' into an EmulatorSpec."""
    fields: dict[str, str] = {}
    for pair in value.split(","):
        pair = pair.strip()
        if "=" not in pair:
            raise argparse.ArgumentTypeError(
                f"Invalid emulator spec: expected key=value, got {pair!r}"
            )
        key, val = pair.split("=", 1)
        key = key.strip()
        if key not in ("port", "delay", "file", "type"):
            raise argparse.ArgumentTypeError(
                f"Unknown emulator key {key!r}. Valid keys: port, delay, file, type"
            )
        fields[key] = val.strip()

    if "port" not in fields:
        raise argparse.ArgumentTypeError("Emulator spec must include 'port'")

    try:
        port = int(fields["port"])
    except ValueError:
        raise argparse.ArgumentTypeError(f"Invalid port: {fields['port']!r}")

    delay = EMULATOR_DEFAULT_DELAY
    if "delay" in fields:
        try:
            delay = int(fields["delay"])
        except ValueError:
            raise argparse.ArgumentTypeError(f"Invalid delay: {fields['delay']!r}")

    read_type = fields.get("type", "raw")
    if read_type not in EMULATOR_VALID_TYPES:
        raise argparse.ArgumentTypeError(
            f"Invalid type {read_type!r}. Valid types: {', '.join(EMULATOR_VALID_TYPES)}"
        )

    return EmulatorSpec(
        port=port,
        delay=delay,
        file=fields.get("file"),
        read_type=read_type,
    )


PANES_BEFORE_EMULATOR = [
    ("Postgres",  f"docker logs -f {PG_CONTAINER}"),
    (
        "Server",
        f"DATABASE_URL=postgres://{PG_USER}:{PG_PASSWORD}@localhost:{PG_PORT}/{PG_DB} "
        f"BIND_ADDR=0.0.0.0:8080 LOG_LEVEL=debug cargo run -p server",
    ),
]

PANES_AFTER_EMULATOR = [
    ("Forwarder", f"cargo run -p forwarder -- --config {FORWARDER_TOML_PATH}"),
    ("Receiver",     "cargo run -p receiver"),
    ("Dashboard",    "cd apps/dashboard && npm run dev"),
    ("Receiver UI",  "cd apps/receiver-ui && npm run dev"),
]

FORWARDER_TOML_HEADER = f"""\
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
"""


def build_forwarder_toml(emulators: list[EmulatorSpec]) -> str:
    readers = "\n".join(e.to_reader_toml() for e in emulators)
    return FORWARDER_TOML_HEADER + "\n" + readers


def build_panes(emulators: list[EmulatorSpec]) -> list[tuple[str, str]]:
    if len(emulators) == 1:
        emu_panes = [("Emulator", emulators[0].to_cmd())]
    else:
        emu_panes = [
            (f"Emulator {i + 1}", e.to_cmd()) for i, e in enumerate(emulators)
        ]
    return PANES_BEFORE_EMULATOR + emu_panes + PANES_AFTER_EMULATOR

console = Console()


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def sha256_hex(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def clear() -> None:
    """Remove all dev artifacts: tmux session, Docker container, tmp files."""
    console.print(Panel.fit(
        "[bold red]Rusty Timer Dev Cleanup[/bold red]\n"
        "Removing dev environment artifacts…",
        border_style="red",
    ))

    # 1. Kill tmux session
    if shutil.which("tmux"):
        result = subprocess.run(
            ["tmux", "kill-session", "-t", "rusty-dev"],
            capture_output=True,
        )
        if result.returncode == 0:
            console.print("  [green]Killed[/green] tmux session: rusty-dev")
        else:
            console.print("  [dim]No tmux session found[/dim]")
    else:
        console.print("  [dim]tmux not installed — skipping[/dim]")

    # 2. Stop and remove Docker container
    if shutil.which("docker"):
        result = subprocess.run(
            ["docker", "rm", "-f", PG_CONTAINER],
            capture_output=True,
        )
        if result.returncode == 0:
            console.print(f"  [green]Removed[/green] Docker container: {PG_CONTAINER}")
        else:
            console.print(f"  [dim]No Docker container found: {PG_CONTAINER}[/dim]")
    else:
        console.print("  [dim]docker not installed — skipping[/dim]")

    # 3. Remove tmp directory
    if TMP_DIR.exists():
        shutil.rmtree(TMP_DIR)
        console.print(f"  [green]Removed[/green] {TMP_DIR}")
    else:
        console.print(f"  [dim]No tmp directory found: {TMP_DIR}[/dim]")

    console.print("\n[bold green]Cleanup complete.[/bold green]")


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


def apply_migrations() -> None:
    """Apply the server schema migration via psql, recording it in _sqlx_migrations.

    sqlx validates on startup that each migration's SHA-384 checksum matches the
    recorded value. By inserting the tracking row here, the server skips re-applying
    the migration and avoids the "relation already exists" error.
    """
    console.print("Applying database migrations…")
    migration_path = REPO_ROOT / "services" / "server" / "migrations" / "0001_init.sql"
    migration_bytes = migration_path.read_bytes()

    # Step 1: apply the migration SQL.
    result = subprocess.run(
        ["docker", "exec", "-i", PG_CONTAINER, "psql", "-U", PG_USER, "-d", PG_DB],
        input=migration_bytes.decode(),
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        console.print(f"[red]Migration failed (psql returned {result.returncode}):[/red]\n{result.stderr}")
        sys.exit(1)

    # Step 2: record the migration in _sqlx_migrations so the server does not re-run it.
    # sqlx 0.8 uses SHA-384 of the raw migration file bytes as the checksum.
    checksum_hex = hashlib.sha384(migration_bytes).hexdigest()
    tracking_sql = (
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations ("
        "  version BIGINT PRIMARY KEY,"
        "  description TEXT NOT NULL,"
        "  installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),"
        "  success BOOLEAN NOT NULL,"
        "  checksum BYTEA NOT NULL,"
        "  execution_time BIGINT NOT NULL"
        ");"
        f"INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time)"
        f" VALUES (1, 'init', NOW(), true, decode('{checksum_hex}', 'hex'), 0)"
        f" ON CONFLICT (version) DO NOTHING;"
    )
    result = subprocess.run(
        ["docker", "exec", "-i", PG_CONTAINER, "psql", "-U", PG_USER, "-d", PG_DB],
        input=tracking_sql,
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        console.print(f"[red]Migration tracking record failed:[/red]\n{result.stderr}")
        sys.exit(1)

    console.print("[green]✓[/green] Migrations applied")


def write_config_files(emulators: list[EmulatorSpec]) -> None:
    console.print("[bold]Writing config files…[/bold]")
    TMP_DIR.mkdir(parents=True, exist_ok=True)
    FORWARDER_TOML_PATH.write_text(build_forwarder_toml(emulators))
    console.print(f"  [green]Wrote[/green] {FORWARDER_TOML_PATH}")
    FORWARDER_TOKEN_PATH.write_text(FORWARDER_TOKEN_TEXT)
    console.print(f"  [green]Wrote[/green] {FORWARDER_TOKEN_PATH}")
    RECEIVER_TOKEN_PATH.write_text(RECEIVER_TOKEN_TEXT)
    console.print(f"  [green]Wrote[/green] {RECEIVER_TOKEN_PATH}")


def seed_tokens() -> None:
    console.print("[bold]Seeding dev tokens into Postgres…[/bold]")
    for token_text, device_type, device_id in [
        (FORWARDER_TOKEN_TEXT, "forwarder", FORWARDER_DEVICE_ID),
        (RECEIVER_TOKEN_TEXT,  "receiver",  RECEIVER_DEVICE_ID),
    ]:
        hex_hash = sha256_hex(token_text)
        # hex_hash and device_id are [0-9a-f] / known safe strings; device_type is a
        # hardcoded literal.  None comes from external input, so f-string interpolation is safe.
        sql = (
            f"INSERT INTO device_tokens (token_hash, device_type, device_id) "
            f"VALUES (decode('{hex_hash}', 'hex'), '{device_type}', '{device_id}') "
            f"ON CONFLICT (token_hash) DO UPDATE SET device_id = EXCLUDED.device_id;"
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
        console.print(f"  [green]Seeded[/green] {device_type} token (sha256={hex_hash[:16]}… device_id={device_id})")


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
    for app_name in ("dashboard", "receiver-ui"):
        app_dir = REPO_ROOT / "apps" / app_name
        if (app_dir / "node_modules").exists():
            console.print(f"[dim]node_modules present in apps/{app_name} — skipping npm install.[/dim]")
        else:
            console.print(f"[bold]Running npm install in apps/{app_name}…[/bold]")
            subprocess.run(["npm", "install"], check=True, cwd=app_dir)
            console.print("  [green]npm install complete.[/green]")


def setup(skip_build: bool = False, emulators: list[EmulatorSpec] | None = None) -> None:
    check_prereqs()
    start_postgres()
    wait_for_postgres()
    apply_migrations()
    write_config_files(emulators or [EmulatorSpec(port=EMULATOR_DEFAULT_PORT)])
    seed_tokens()
    build_rust(skip_build=skip_build)
    npm_install()


# ---------------------------------------------------------------------------
# tmux launcher
# ---------------------------------------------------------------------------

def launch_tmux(panes: list[tuple[str, str]]) -> None:
    session = "rusty-dev"
    subprocess.run(["tmux", "kill-session", "-t", session], capture_output=True)
    subprocess.run(["tmux", "new-session", "-d", "-s", session], check=True)
    for _ in range(len(panes) - 1):
        subprocess.run(["tmux", "split-window", "-t", session], check=True)
        subprocess.run(["tmux", "select-layout", "-t", session, "tiled"], check=True)
    subprocess.run(["tmux", "select-layout", "-t", session, "tiled"], check=True)
    for i, (title, cmd) in enumerate(panes):
        pane = f"{session}:0.{i}"
        subprocess.run(["tmux", "select-pane", "-t", pane, "-T", title], check=True)
        full_cmd = f'cd "{REPO_ROOT}" && {cmd}'
        subprocess.run(["tmux", "send-keys", "-t", pane, full_cmd, "Enter"], check=True)
    subprocess.run(["tmux", "attach-session", "-t", session])


# ---------------------------------------------------------------------------
# iTerm2 launcher (iterm2 Python API)
# ---------------------------------------------------------------------------

def launch_iterm2(panes: list[tuple[str, str]]) -> None:
    import iterm2
    iterm2.run_until_complete(lambda conn: _iterm2_async(conn, panes))


async def _iterm2_async(connection, panes: list[tuple[str, str]]) -> None:
    import iterm2
    await iterm2.async_get_app(connection)
    window = await iterm2.Window.async_create(connection)
    tab = window.tabs[0]
    first_session = tab.sessions[0]

    # Create additional sessions by splitting
    sessions = [first_session]
    for _ in range(len(panes) - 1):
        # Alternate vertical/horizontal splits for a reasonable layout
        new_session = await sessions[-1].async_split_pane(
            vertical=(len(sessions) % 2 == 0)
        )
        sessions.append(new_session)

    for session, (title, cmd) in zip(sessions, panes):
        await session.async_set_name(title)
        await session.async_send_text(f'cd "{REPO_ROOT}" && {cmd}\n')


# ---------------------------------------------------------------------------
# Detect and launch
# ---------------------------------------------------------------------------

def detect_and_launch(emulators: list[EmulatorSpec]) -> None:
    panes = build_panes(emulators)
    if shutil.which("tmux"):
        console.print("[blue]Multiplexer:[/blue] tmux")
        launch_tmux(panes)
    elif Path("/Applications/iTerm.app").exists():
        console.print("[blue]Multiplexer:[/blue] iTerm2")
        console.print("[dim]Note: requires Preferences → General → Magic → Enable Python API[/dim]")
        launch_iterm2(panes)
    else:
        console.print("[red]No multiplexer found.[/red]")
        console.print("Install tmux:  brew install tmux")
        console.print("Or install iTerm2: https://iterm2.com")
        sys.exit(1)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Rusty Timer Dev Launcher")
    parser.add_argument("--no-build", action="store_true", help="Skip the Rust build step")
    parser.add_argument("--clear", action="store_true", help="Tear down dev artifacts and exit")
    parser.add_argument(
        "--emulator", action="append", type=parse_emulator_spec, metavar="SPEC",
        help=(
            "Emulator instance spec as key=value pairs: port=N,delay=MS,file=PATH,type=raw|fsls. "
            "Repeatable for multiple emulators. Default: single emulator on port 10001."
        ),
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()

    if args.clear:
        clear()
        return

    emulators: list[EmulatorSpec] = args.emulator or [EmulatorSpec(port=EMULATOR_DEFAULT_PORT)]

    # Validate no duplicate ports
    ports = [e.port for e in emulators]
    if len(ports) != len(set(ports)):
        console.print("[red]Error: duplicate emulator ports[/red]")
        sys.exit(1)

    console.print(Panel.fit(
        "[bold cyan]Rusty Timer Dev Launcher[/bold cyan]\n"
        "Setting up local dev environment…",
        border_style="cyan",
    ))
    setup(skip_build=args.no_build, emulators=emulators)
    console.print("\n[bold green]Setup complete — launching services…[/bold green]\n")
    detect_and_launch(emulators)


if __name__ == "__main__":
    main()
