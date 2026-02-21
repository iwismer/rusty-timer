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
    uv run scripts/dev.py --log-level debug
    uv run scripts/dev.py --emulator port=10001,delay=500,file=start.txt
    uv run scripts/dev.py --emulator port=10001 --emulator port=10002,delay=500
    uv run scripts/dev.py --bibchip test_assets/bibchip/large.txt --ppl test_assets/ppl/large.ppl
"""

import argparse
import hashlib
import json
import math
import shlex
import shutil
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
import uuid
from dataclasses import dataclass
from pathlib import Path

from rich.console import Console
from rich.panel import Panel

REPO_ROOT = Path(__file__).resolve().parent.parent

TMP_DIR = Path("/tmp/rusty-timer-dev")
FORWARDER_TOML_PATH = TMP_DIR / "forwarder.toml"
FORWARDER_TOKEN_PATH = TMP_DIR / "forwarder-token.txt"
RECEIVER_TOKEN_PATH = TMP_DIR / "receiver-token.txt"
RECEIVER_CONFIG_SCRIPT_PATH = TMP_DIR / "configure-receiver.sh"
FORWARDER_JOURNAL_PATH = TMP_DIR / "forwarder.sqlite3"
ITERM_WINDOW_ID_PATH = TMP_DIR / "iterm-window-id.txt"

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
MIN_PORT = 1
MAX_PORT = 65535
FALLBACK_OFFSET = 1000
RECEIVER_DYNAMIC_MIN_PORT = 12000


@dataclass
class EmulatorSpec:
    port: int
    delay: int = EMULATOR_DEFAULT_DELAY
    file: str | None = None
    read_type: str = "raw"

    def __post_init__(self) -> None:
        if self.read_type not in EMULATOR_VALID_TYPES:
            raise ValueError(f"Invalid read_type {self.read_type!r}")

    def to_cmd(self) -> str:
        cmd = f"./target/debug/emulator --port {self.port} --delay {self.delay} --type {self.read_type}"
        if self.file:
            cmd += f" --file {shlex.quote(self.file)}"
        return cmd

    def to_reader_toml(self) -> str:
        return (
            f"[[readers]]\n"
            f'target              = "127.0.0.1:{self.port}"\n'
            f"enabled             = true\n"
            f"local_fallback_port = {self.port + FALLBACK_OFFSET}\n"
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
    if not (MIN_PORT <= port <= MAX_PORT):
        raise argparse.ArgumentTypeError(
            f"Invalid port {port}: out of range {MIN_PORT}..{MAX_PORT}"
        )
    fallback_port = port + FALLBACK_OFFSET
    if fallback_port > MAX_PORT:
        raise argparse.ArgumentTypeError(
            f"Invalid port {port}: fallback port {fallback_port} exceeds {MAX_PORT}"
        )

    delay = EMULATOR_DEFAULT_DELAY
    if "delay" in fields:
        try:
            delay = int(fields["delay"])
        except ValueError:
            raise argparse.ArgumentTypeError(f"Invalid delay: {fields['delay']!r}")
        if delay < 0:
            raise argparse.ArgumentTypeError(
                f"Invalid delay {delay}: must be non-negative"
            )

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


PANES_AFTER_EMULATOR = [
    ("Forwarder", f"./target/debug/forwarder --config {FORWARDER_TOML_PATH}"),
]

FORWARDER_TOML_HEADER = f"""\
schema_version = 1

[server]
base_url = "http://127.0.0.1:8080"

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


def build_panes(emulators: list[EmulatorSpec], *, log_level: str = "info") -> list[tuple[str, str]]:
    dashboard_build_dir = REPO_ROOT / "apps" / "server-ui" / "build"
    dashboard_env = ""
    if dashboard_build_dir.is_dir():
        dashboard_env = f"DASHBOARD_DIR={shlex.quote(str(dashboard_build_dir))} "

    panes_before_emulator = [
        ("Postgres", f"docker logs -f {PG_CONTAINER}"),
        (
            "Server",
            f"DATABASE_URL=postgres://{PG_USER}:{PG_PASSWORD}@localhost:{PG_PORT}/{PG_DB} "
            f"{dashboard_env}"
            f"BIND_ADDR=0.0.0.0:8080 LOG_LEVEL={shlex.quote(log_level)} ./target/debug/server",
        ),
    ]

    if len(emulators) == 1:
        emu_panes = [("Emulator", emulators[0].to_cmd())]
    else:
        emu_panes = [
            (f"Emulator {i + 1}", e.to_cmd()) for i, e in enumerate(emulators)
        ]
    panes_after_emulator = PANES_AFTER_EMULATOR + [
        ("Receiver", f"RUST_LOG={shlex.quote(log_level)} ./target/debug/receiver"),
    ]
    return panes_before_emulator + emu_panes + panes_after_emulator

console = Console()
stderr_console = Console(stderr=True)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def sha256_hex(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def fnv1a_64(text: str) -> int:
    """Stable 64-bit FNV-1a hash."""
    h = 0xCBF29CE484222325
    for b in text.encode():
        h ^= b
        h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def receiver_default_local_port(reader_ip: str) -> int | None:
    """Mirror receiver default port resolution for a stream reader_ip."""
    ip_part = reader_ip
    source_port: int | None = None
    if ":" in reader_ip:
        ip_part, port_text = reader_ip.rsplit(":", 1)
        try:
            source_port = int(port_text)
        except ValueError:
            return None
        if not (MIN_PORT <= source_port <= MAX_PORT):
            return None

    parts = ip_part.split(".")
    if len(parts) != 4:
        return None
    try:
        last = int(parts[3])
    except ValueError:
        return None
    if not (0 <= last <= 255):
        return None

    legacy = 10000 + last
    if source_port is None or source_port == 10000:
        return legacy

    span = MAX_PORT - RECEIVER_DYNAMIC_MIN_PORT + 1
    return RECEIVER_DYNAMIC_MIN_PORT + (fnv1a_64(reader_ip) % span)


def generate_reads_from_bibchip(bibchip_path: Path) -> Path:
    """Generate an emulator-compatible reads file from a bibchip CSV.

    Produces one valid IPICO RAW read per chip, spread across a simulated
    race finish window (starting at 10:32:00 on 2025-06-15, ~14s apart).
    Returns the path to the generated file in TMP_DIR.
    """
    chips: list[str] = []
    text = bibchip_path.read_text(encoding="utf-8")
    for line in text.splitlines():
        line = line.strip()
        if not line or not line[0].isdigit():
            continue
        parts = line.split(",")
        if len(parts) >= 2 and parts[1].strip():
            chips.append(parts[1].strip())

    if not chips:
        console.print(f"[yellow]Warning:[/yellow] No chips found in {bibchip_path}")
        return bibchip_path  # fallback — won't be valid reads, but won't crash

    TMP_DIR.mkdir(parents=True, exist_ok=True)
    out_path = TMP_DIR / "generated-reads.txt"
    reads: list[str] = []
    base_min, base_sec = 32, 0

    for i, chip_id in enumerate(chips):
        total_seconds = base_sec + i * 14
        minutes = base_min + total_seconds // 60
        seconds = total_seconds % 60
        hours = 10 + minutes // 60
        minutes = minutes % 60
        centiseconds = (i * 7) % 100

        read = (
            f"aa00{chip_id}0001"
            f"{25:02d}{6:02d}{15:02d}"
            f"{hours:02d}{minutes:02d}{seconds:02d}{centiseconds:02x}"
        )
        checksum = sum(ord(c) for c in read[2:34]) % 256
        read += f"{checksum:02x}"
        reads.append(read)

    out_path.write_text("\n".join(reads) + "\n")
    console.print(f"  [green]Generated[/green] {len(reads)} reads → {out_path}")
    return out_path


def _multipart_upload(url: str, file_bytes: bytes, filename: str) -> bytes:
    """POST a file as multipart/form-data using only stdlib."""
    boundary = uuid.uuid4().hex
    body = (
        f"--{boundary}\r\n"
        f'Content-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        f"Content-Type: application/octet-stream\r\n"
        f"\r\n"
    ).encode() + file_bytes + f"\r\n--{boundary}--\r\n".encode()

    req = urllib.request.Request(
        url,
        data=body,
        headers={"Content-Type": f"multipart/form-data; boundary={boundary}"},
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=10) as resp:
        return resp.read()


def setup_race_data(bibchip_path: Path | None, ppl_path: Path | None) -> None:
    """Wait for the server, then create a race and upload bibchip/ppl files.

    Writes a diagnostic log to TMP_DIR/race-setup.log so results are visible
    even when tmux has taken over the terminal.
    """
    log_path = TMP_DIR / "race-setup.log"
    log_lines: list[str] = []

    def log(msg: str) -> None:
        log_lines.append(msg)
        stderr_console.print(msg)

    try:
        _do_setup_race_data(bibchip_path, ppl_path, log)
    except Exception as exc:
        log(f"[red]Race data setup crashed:[/red] {type(exc).__name__}: {exc}")
    finally:
        TMP_DIR.mkdir(parents=True, exist_ok=True)
        log_path.write_text("\n".join(log_lines) + "\n")


def _do_setup_race_data(
    bibchip_path: Path | None,
    ppl_path: Path | None,
    log: callable,
) -> None:
    base = "http://127.0.0.1:8080"

    # Wait for server health
    for attempt in range(60):
        try:
            urllib.request.urlopen(f"{base}/healthz", timeout=2)
            break
        except (urllib.error.URLError, OSError):
            if attempt == 59:
                log(
                    "[yellow]Warning:[/yellow] Server not ready after 60s — "
                    "skipping race data setup."
                )
            time.sleep(1)
    else:
        return

    log(f"  Server ready, setting up race data…")

    # Create race
    race_name = "Dev Race"
    payload = json.dumps({"name": race_name}).encode()
    req = urllib.request.Request(
        f"{base}/api/v1/races",
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            race = json.loads(resp.read())
        race_id = race["race_id"]
    except (urllib.error.URLError, OSError, KeyError, json.JSONDecodeError) as exc:
        log(f"[yellow]Warning:[/yellow] Failed to create race: {exc}")
        return

    log(f"  [green]Created race:[/green] {race_name} ({race_id})")

    # Upload bibchip
    if bibchip_path:
        try:
            resp_body = _multipart_upload(
                f"{base}/api/v1/races/{race_id}/chips/upload",
                bibchip_path.read_bytes(),
                bibchip_path.name,
            )
            log(f"  [green]Uploaded bibchip:[/green] {bibchip_path.name} → {resp_body.decode()}")
        except (urllib.error.URLError, OSError) as exc:
            log(f"[yellow]Warning:[/yellow] Bibchip upload failed: {exc}")

    # Upload PPL
    if ppl_path:
        try:
            resp_body = _multipart_upload(
                f"{base}/api/v1/races/{race_id}/participants/upload",
                ppl_path.read_bytes(),
                ppl_path.name,
            )
            log(f"  [green]Uploaded PPL:[/green] {ppl_path.name} → {resp_body.decode()}")
        except (urllib.error.URLError, OSError) as exc:
            log(f"[yellow]Warning:[/yellow] PPL upload failed: {exc}")


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
    for tool in ("docker", "cargo", "npm", "curl"):
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
    """Apply all server schema migrations via psql, recording them in _sqlx_migrations.

    Discovers every *.sql file under services/server/migrations/, applies them in
    sorted order, and records each in the _sqlx_migrations tracking table so that
    sqlx's compile-time checks and runtime migration validation both pass.

    sqlx 0.8 uses SHA-384 of the raw migration file bytes as the checksum.
    """
    console.print("[bold]Applying database migrations…[/bold]")
    migrations_dir = REPO_ROOT / "services" / "server" / "migrations"
    migration_files = sorted(migrations_dir.glob("*.sql"))

    if not migration_files:
        console.print("  [dim]No migration files found.[/dim]")
        return

    psql_base = ["docker", "exec", "-i", PG_CONTAINER, "psql", "-U", PG_USER, "-d", PG_DB]

    # Ensure tracking table exists.
    tracking_ddl = (
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations ("
        "  version BIGINT PRIMARY KEY,"
        "  description TEXT NOT NULL,"
        "  installed_on TIMESTAMPTZ NOT NULL DEFAULT now(),"
        "  success BOOLEAN NOT NULL,"
        "  checksum BYTEA NOT NULL,"
        "  execution_time BIGINT NOT NULL"
        ");"
    )
    subprocess.run(psql_base, input=tracking_ddl, capture_output=True, text=True, check=True)

    # Determine which migrations have already been applied.
    result = subprocess.run(
        ["docker", "exec", PG_CONTAINER, "psql", "-U", PG_USER, "-d", PG_DB,
         "-tAc", "SELECT version FROM _sqlx_migrations"],
        capture_output=True, text=True,
    )
    applied: set[int] = set()
    if result.returncode == 0:
        applied = {int(v.strip()) for v in result.stdout.strip().splitlines() if v.strip()}

    for mf in migration_files:
        # Filename format: "0002_epoch_metrics.sql" → version=2, description="epoch_metrics"
        stem = mf.stem
        parts = stem.split("_", 1)
        version = int(parts[0])
        description = parts[1] if len(parts) > 1 else stem

        if version in applied:
            console.print(f"  [dim]Already applied:[/dim] {mf.name}")
            continue

        migration_bytes = mf.read_bytes()
        checksum_hex = hashlib.sha384(migration_bytes).hexdigest()

        # Apply the migration SQL.
        result = subprocess.run(
            psql_base, input=migration_bytes.decode(), capture_output=True, text=True,
        )
        if result.returncode != 0:
            console.print(f"[red]Migration {mf.name} failed:[/red]\n{result.stderr}")
            sys.exit(1)

        # Record in _sqlx_migrations so the server does not re-run it.
        tracking_sql = (
            f"INSERT INTO _sqlx_migrations (version, description, installed_on, success, checksum, execution_time)"
            f" VALUES ({version}, '{description}', NOW(), true, decode('{checksum_hex}', 'hex'), 0)"
            f" ON CONFLICT (version) DO NOTHING;"
        )
        result = subprocess.run(
            psql_base, input=tracking_sql, capture_output=True, text=True,
        )
        if result.returncode != 0:
            console.print(f"[red]Migration tracking for {mf.name} failed:[/red]\n{result.stderr}")
            sys.exit(1)

        console.print(f"  [green]Applied:[/green] {mf.name}")

    console.print("[green]✓[/green] Migrations up to date")


def write_config_files(emulators: list[EmulatorSpec]) -> None:
    console.print("[bold]Writing config files…[/bold]")
    TMP_DIR.mkdir(parents=True, exist_ok=True)
    FORWARDER_TOML_PATH.write_text(build_forwarder_toml(emulators))
    console.print(f"  [green]Wrote[/green] {FORWARDER_TOML_PATH}")
    FORWARDER_TOKEN_PATH.write_text(FORWARDER_TOKEN_TEXT)
    console.print(f"  [green]Wrote[/green] {FORWARDER_TOKEN_PATH}")
    RECEIVER_TOKEN_PATH.write_text(RECEIVER_TOKEN_TEXT)
    console.print(f"  [green]Wrote[/green] {RECEIVER_TOKEN_PATH}")
    RECEIVER_CONFIG_SCRIPT_PATH.write_text(
        "#!/usr/bin/env bash\n"
        "# Auto-generated by dev.py — configure the receiver for local dev.\n"
        "set -euo pipefail\n"
        'BASE=http://127.0.0.1:9090\n'
        'echo "Waiting for receiver control API…"\n'
        'for i in $(seq 1 60); do\n'
        '  curl -sf "$BASE/healthz" >/dev/null 2>&1 && break\n'
        '  sleep 1\n'
        'done\n'
        'curl -sf -X PUT "$BASE/api/v1/profile" \\\n'
        "  -H 'Content-Type: application/json' \\\n"
        f"  -d '{{\"server_url\":\"ws://127.0.0.1:8080\",\"token\":\"{RECEIVER_TOKEN_TEXT}\",\"log_level\":\"info\"}}'\n"
        'echo "Profile set"\n'
        'curl -sf -X POST "$BASE/api/v1/connect"\n'
        'echo "Connected"\n'
    )
    RECEIVER_CONFIG_SCRIPT_PATH.chmod(0o755)
    console.print(f"  [green]Wrote[/green] {RECEIVER_CONFIG_SCRIPT_PATH}")


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
        ["cargo", "build", "-p", "server", "-p", "forwarder", "--features", "forwarder/embed-ui", "-p", "receiver", "--features", "receiver/embed-ui", "-p", "emulator"],
        check=True,
        cwd=REPO_ROOT,
    )
    console.print("  [green]Build complete.[/green]")


def npm_install() -> None:
    console.print("[bold]Running npm install in workspace root…[/bold]")
    subprocess.run(["npm", "install"], check=True, cwd=REPO_ROOT)
    console.print("  [green]npm install complete.[/green]")


def build_dashboard(skip_build: bool = False) -> None:
    if skip_build:
        console.print("[dim]Skipping dashboard build (--no-build)[/dim]")
        return
    console.print("[bold]Ensuring dashboard workspace dependencies…[/bold]")
    subprocess.run(
        ["npm", "install", "--workspace=apps/server-ui"],
        check=True,
        cwd=REPO_ROOT,
    )
    console.print("[bold]Building dashboard…[/bold]")
    subprocess.run(
        ["npm", "run", "build", "--workspace=apps/server-ui"],
        check=True,
        cwd=REPO_ROOT,
    )
    console.print("  [green]Dashboard build complete.[/green]")


def setup(skip_build: bool = False, emulators: list[EmulatorSpec] | None = None) -> None:
    check_prereqs()
    start_postgres()
    wait_for_postgres()
    apply_migrations()
    write_config_files(emulators or [EmulatorSpec(port=EMULATOR_DEFAULT_PORT)])
    seed_tokens()
    npm_install()
    build_dashboard(skip_build=skip_build)
    build_rust(skip_build=skip_build)


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


async def _split_n(session, n: int, *, vertical: bool) -> list:
    """Split one iTerm2 session into *n* sub-panes along the given axis.

    Uses balanced binary splitting so that all resulting panes are
    approximately equal in size.  Returns sessions in visual order
    (top-to-bottom for horizontal, left-to-right for vertical).
    """
    if n <= 1:
        return [session]
    first_half = n // 2
    second_half = n - first_half
    new_session = await session.async_split_pane(vertical=vertical)
    first_sessions = await _split_n(session, first_half, vertical=vertical)
    second_sessions = await _split_n(new_session, second_half, vertical=vertical)
    return first_sessions + second_sessions


async def _iterm2_async(connection, panes: list[tuple[str, str]]) -> None:
    import asyncio
    import iterm2

    # async_get_app subscribes to iTerm2 layout/focus notifications.  When
    # run_until_complete finishes and closes the websocket those handlers fire
    # on a dead connection, producing noisy "Task exception was never retrieved"
    # ConnectionClosedError tracebacks.  Suppress them here.
    loop = asyncio.get_event_loop()
    _orig = loop.get_exception_handler()

    def _quiet(loop, ctx):
        exc = ctx.get("exception")
        if exc and "ConnectionClosed" in type(exc).__name__:
            return
        if _orig:
            _orig(loop, ctx)
        else:
            loop.default_exception_handler(ctx)

    loop.set_exception_handler(_quiet)

    await iterm2.async_get_app(connection)
    window = await iterm2.Window.async_create(connection)
    TMP_DIR.mkdir(parents=True, exist_ok=True)
    ITERM_WINDOW_ID_PATH.write_text(window.window_id)
    tab = window.tabs[0]
    root = tab.sessions[0]

    n = len(panes)
    if n <= 1:
        sessions = [root]
    else:
        cols = min(2, n)

        # Phase 1: split into columns
        col_roots = await _split_n(root, cols, vertical=True)

        # Phase 2: determine rows per column (left column gets extra if odd)
        rows_per_col: list[int] = []
        remaining = n
        for c in range(cols):
            rows_here = math.ceil(remaining / (cols - c))
            rows_per_col.append(rows_here)
            remaining -= rows_here

        # Phase 3: split each column into rows
        col_sessions: list[list] = []
        for c in range(cols):
            row_sessions = await _split_n(
                col_roots[c], rows_per_col[c], vertical=False
            )
            col_sessions.append(row_sessions)

        # Interleave columns for row-major order
        sessions = []
        for r in range(max(rows_per_col)):
            for c in range(cols):
                if r < rows_per_col[c]:
                    sessions.append(col_sessions[c][r])

    for session, (title, cmd) in zip(sessions, panes):
        await session.async_set_name(title)
        await session.async_send_text(f'cd "{REPO_ROOT}" && {cmd}\n')


# ---------------------------------------------------------------------------
# Detect and launch
# ---------------------------------------------------------------------------

def configure_receiver_dev() -> None:
    """Poll the receiver control API and configure the dev profile.

    Logs warnings to stderr on failure — the user can re-run
    /tmp/rusty-timer-dev/configure-receiver.sh manually if needed.
    """
    base = "http://127.0.0.1:9090"

    for attempt in range(60):
        try:
            urllib.request.urlopen(f"{base}/healthz", timeout=2)
            break
        except (urllib.error.URLError, OSError):
            if attempt == 59:  # Only log on final attempt
                stderr_console.print(
                    f"[yellow]Warning:[/yellow] Receiver control API not ready after 60s. "
                    f"Run /tmp/rusty-timer-dev/configure-receiver.sh manually.",
                )
            time.sleep(1)
    else:
        return

    profile_payload = json.dumps({
        "server_url": "ws://127.0.0.1:8080",
        "token": RECEIVER_TOKEN_TEXT,
        "log_level": "info",
    }).encode()
    req = urllib.request.Request(
        f"{base}/api/v1/profile",
        data=profile_payload,
        headers={"Content-Type": "application/json"},
        method="PUT",
    )
    try:
        urllib.request.urlopen(req, timeout=5)
    except (urllib.error.URLError, OSError):
        stderr_console.print(
            f"[yellow]Warning:[/yellow] Failed to set receiver profile. "
            f"Run /tmp/rusty-timer-dev/configure-receiver.sh manually.",
        )
        return

    req = urllib.request.Request(f"{base}/api/v1/connect", data=b"", method="POST")
    try:
        urllib.request.urlopen(req, timeout=5)
    except (urllib.error.URLError, OSError):
        stderr_console.print(
            f"[yellow]Warning:[/yellow] Failed to connect receiver. "
            f"Run /tmp/rusty-timer-dev/configure-receiver.sh manually.",
        )
        return


def start_receiver_auto_config() -> threading.Thread:
    """Run receiver auto-config in a background thread.

    Returns the thread so the caller can join it before exit.
    """
    thread = threading.Thread(
        target=configure_receiver_dev,
        name="receiver-auto-config",
    )
    thread.start()
    return thread


DEV_BINARIES = ("server", "forwarder", "receiver", "emulator")


def close_iterm2_window() -> None:
    """Close the iTerm2 dev window using the saved window ID."""
    if not ITERM_WINDOW_ID_PATH.exists():
        return
    window_id = ITERM_WINDOW_ID_PATH.read_text().strip()
    ITERM_WINDOW_ID_PATH.unlink(missing_ok=True)
    if not window_id:
        return

    try:
        import iterm2
    except ImportError:
        return

    async def _close(connection):
        import asyncio

        loop = asyncio.get_event_loop()
        _orig = loop.get_exception_handler()

        def _quiet(loop, ctx):
            exc = ctx.get("exception")
            if exc and "ConnectionClosed" in type(exc).__name__:
                return
            if _orig:
                _orig(loop, ctx)
            else:
                loop.default_exception_handler(ctx)

        loop.set_exception_handler(_quiet)

        app = await iterm2.async_get_app(connection)
        for window in app.windows:
            if window.window_id == window_id:
                await window.async_close(force=True)
                break

    try:
        iterm2.run_until_complete(_close)
    except Exception:
        pass


SERVER_PORT = 8080


def _listener_pids(port: int) -> list[int]:
    """Return PIDs currently listening on the given TCP port."""
    result = subprocess.run(
        ["lsof", "-t", "-sTCP:LISTEN", "-i", f":{port}"],
        capture_output=True,
        text=True,
    )
    pids: list[int] = []
    for raw_pid in result.stdout.strip().split():
        try:
            pids.append(int(raw_pid))
        except ValueError:
            continue
    return pids


def _pid_command(pid: int) -> str:
    """Return the full command line for a process, or empty string on failure."""
    result = subprocess.run(
        ["ps", "-p", str(pid), "-o", "command="],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        return ""
    return result.stdout.strip()


def _has_saved_iterm_window_id() -> bool:
    """Return True when we have a persisted iTerm dev window id."""
    if not ITERM_WINDOW_ID_PATH.exists():
        return False
    try:
        return bool(ITERM_WINDOW_ID_PATH.read_text().strip())
    except OSError:
        return False


def _is_dev_server_command(command: str) -> bool:
    """Return True when command line looks like this repo's server binary."""
    if "target/debug/server" in command or "target/release/server" in command:
        return True

    # On macOS, `ps -o command=` may return only the executable basename.
    # If we also have a saved iTerm window id from this script, treat `server`
    # as our dev server.
    try:
        argv0 = shlex.split(command)[0]
    except (ValueError, IndexError):
        argv0 = command.strip().split(" ", 1)[0] if command.strip() else ""

    return Path(argv0).name == "server" and _has_saved_iterm_window_id()


def _kill_pids(pids: list[int]) -> None:
    """Kill specific process IDs."""
    for pid in pids:
        subprocess.run(["kill", str(pid)], capture_output=True)


def check_existing_instance() -> None:
    """Detect a running dev environment and optionally tear it down."""
    tmux_running = False
    if shutil.which("tmux"):
        result = subprocess.run(
            ["tmux", "has-session", "-t", "rusty-dev"],
            capture_output=True,
        )
        tmux_running = result.returncode == 0

    listener_pids = _listener_pids(SERVER_PORT)
    dev_listener_pids = [
        pid for pid in listener_pids
        if _is_dev_server_command(_pid_command(pid))
    ]
    foreign_listener_pids = [
        pid for pid in listener_pids
        if pid not in dev_listener_pids
    ]

    if not tmux_running and not listener_pids:
        ITERM_WINDOW_ID_PATH.unlink(missing_ok=True)
        return

    if foreign_listener_pids and not dev_listener_pids:
        console.print(
            f"[yellow]Port :{SERVER_PORT} is in use by a non-dev process. "
            f"Refusing to stop it automatically.[/yellow]"
        )
        console.print("[bold]  \\[n] Continue anyway  \\[c] Cancel[/bold]")
        answer = console.input("[bold]> [/bold]").strip().lower()
        if answer in ("c", "cancel"):
            console.print("[dim]Aborted.[/dim]")
            sys.exit(0)
        console.print("[dim]Proceeding without stopping non-dev process.[/dim]")
        return

    parts = []
    if tmux_running:
        parts.append("tmux session 'rusty-dev'")
    if dev_listener_pids:
        parts.append(f"dev server listening on :{SERVER_PORT}")
    elif listener_pids:
        parts.append(f"listener on :{SERVER_PORT}")

    console.print(
        f"[yellow]Existing dev environment detected:[/yellow] {'; '.join(parts)}"
    )
    console.print("[bold]  \\[Y] Kill and restart  \\[n] Continue anyway  \\[c] Cancel[/bold]")
    answer = console.input("[bold]> [/bold]").strip().lower()
    if answer in ("c", "cancel"):
        console.print("[dim]Aborted.[/dim]")
        sys.exit(0)
    if answer not in ("", "y", "yes"):
        console.print("[dim]Proceeding without stopping existing instance.[/dim]")
        return

    if tmux_running:
        subprocess.run(["tmux", "kill-session", "-t", "rusty-dev"], capture_output=True)
        console.print("  [green]Killed[/green] tmux session: rusty-dev")

    close_iterm2_window()

    # Kill the server by port (pkill -f is unreliable on macOS), then mop up
    # any remaining dev binaries with pkill as a safety net.
    if dev_listener_pids:
        _kill_pids(dev_listener_pids)
    for name in DEV_BINARIES:
        subprocess.run(["pkill", "-f", f"target/debug/{name}"], capture_output=True)
    console.print(f"  [green]Killed[/green] dev processes")

    if shutil.which("docker"):
        subprocess.run(["docker", "rm", "-f", PG_CONTAINER], capture_output=True)
        console.print(f"  [green]Stopped[/green] Docker container: {PG_CONTAINER}")


def detect_and_launch(
    emulators: list[EmulatorSpec],
    *,
    bibchip_path: Path | None = None,
    ppl_path: Path | None = None,
    log_level: str = "info",
) -> None:
    panes = build_panes(emulators, log_level=log_level)
    bg_threads: list[threading.Thread] = []
    console.print("[dim]Receiver will be auto-configured with dev profile when ready.[/dim]")
    bg_threads.append(start_receiver_auto_config())
    if bibchip_path or ppl_path:
        console.print("[dim]Race data will be uploaded to server when ready.[/dim]")
        bg_threads.append(start_race_data_setup(bibchip_path, ppl_path))
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
    # Wait for background setup tasks (race data upload, receiver config) to
    # finish before the process exits.  With tmux, the attach call above blocks
    # so these threads will typically have completed already.  With iTerm2 the
    # launcher returns immediately, so we must join explicitly.
    for t in bg_threads:
        t.join()


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------

def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Rusty Timer Dev Launcher")
    parser.add_argument(
        "--no-build",
        action="store_true",
        help="Skip Rust and dashboard build steps",
    )
    parser.add_argument("--clear", action="store_true", help="Tear down dev artifacts and exit")
    parser.add_argument(
        "--emulator", action="append", type=parse_emulator_spec, metavar="SPEC",
        help=(
            "Emulator instance spec as key=value pairs: port=N,delay=MS,file=PATH,type=raw|fsls. "
            "Repeatable for multiple emulators. Default: single emulator on port 10001."
        ),
    )
    parser.add_argument(
        "--bibchip", type=Path, metavar="PATH",
        help=(
            "Path to a bibchip CSV file. Generates emulator reads matching these chips "
            "and uploads the file to a new race after server startup."
        ),
    )
    parser.add_argument(
        "--ppl", type=Path, metavar="PATH",
        help="Path to a PPL participant file. Uploaded to a new race after server startup.",
    )
    parser.add_argument(
        "--log-level", default="info", metavar="LEVEL",
        help="Log level for the server (default: info)",
    )
    return parser.parse_args()


def start_race_data_setup(
    bibchip_path: Path | None, ppl_path: Path | None,
) -> threading.Thread:
    """Run race data setup in a background thread.

    Returns the thread so the caller can join it before exit.
    """
    thread = threading.Thread(
        target=setup_race_data,
        args=(bibchip_path, ppl_path),
        name="race-data-setup",
    )
    thread.start()
    return thread


def main() -> None:
    args = parse_args()

    if args.clear:
        clear()
        return

    # Validate file paths early.
    bibchip_path: Path | None = args.bibchip
    ppl_path: Path | None = args.ppl
    if bibchip_path and not bibchip_path.is_file():
        console.print(f"[red]Error: bibchip file not found: {bibchip_path}[/red]")
        sys.exit(1)
    if ppl_path and not ppl_path.is_file():
        console.print(f"[red]Error: PPL file not found: {ppl_path}[/red]")
        sys.exit(1)

    emulators: list[EmulatorSpec] = args.emulator or [EmulatorSpec(port=EMULATOR_DEFAULT_PORT)]

    # When a bibchip file is provided and the first emulator has no explicit
    # file, generate a reads file from the bibchip and attach it.
    if bibchip_path and emulators[0].file is None:
        console.print("[bold]Generating emulator reads from bibchip…[/bold]")
        reads_path = generate_reads_from_bibchip(bibchip_path)
        emulators[0].file = str(reads_path)

    # Validate no duplicate ports across emulator, fallback, and receiver defaults.
    ports = [e.port for e in emulators]
    fallbacks = [e.port + FALLBACK_OFFSET for e in emulators]
    receiver_defaults: list[int] = []
    for e in emulators:
        stream_key = f"127.0.0.1:{e.port}"
        default_port = receiver_default_local_port(stream_key)
        if default_port is None:
            console.print(f"[red]Error: cannot derive receiver local port for {stream_key}[/red]")
            sys.exit(1)
        receiver_defaults.append(default_port)

    all_ports = ports + fallbacks + receiver_defaults
    if len(all_ports) != len(set(all_ports)):
        console.print(
            "[red]Error: emulator/fallback/receiver default port collision[/red]"
        )
        sys.exit(1)

    check_existing_instance()

    console.print(Panel.fit(
        "[bold cyan]Rusty Timer Dev Launcher[/bold cyan]\n"
        "Setting up local dev environment…",
        border_style="cyan",
    ))
    setup(skip_build=args.no_build, emulators=emulators)
    console.print("\n[bold green]Setup complete — launching services…[/bold green]\n")
    detect_and_launch(emulators, bibchip_path=bibchip_path, ppl_path=ppl_path, log_level=args.log_level)


if __name__ == "__main__":
    main()
