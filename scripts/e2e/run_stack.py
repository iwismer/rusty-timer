#!/usr/bin/env python3
"""Full-stack deterministic loopback E2E orchestrator (Phase 5 T5.4).

Boots the real OS processes for the P2P stack on loopback only, drives a
deterministic, bounded chip-read scenario end-to-end, and asserts backend
facts (not just process startup):

    emulator  --(TCP, verbatim/once)-->  forwarder  --(iroh P2P)-->  receiver-headless
                                                                          |
                                          server  <--(announcer push)--+

The stack is configured for determinism:

* relay disabled, discovery off, injected direct addresses, seeded keys
  (forwarder and receiver share the same seed->endpoint-id derivation);
* the emulator emits a fixed, finite set of byte-for-byte frames once
  (``--verbatim --once``), so every raw frame, seq, and count is known up front;
* a static forwarder allow-list admits exactly the seeded receiver endpoint id.

Assertions (all must be green):

1. Receiver SQLite ``received_events`` has the exact rows/count/raw frames/seqs.
2. Receiver DBF has the exact record count and no duplicates after resume.
3. Receiver durable TCP local proxy replays the exact raw frames to a fresh
   client.
4. Server ``/status`` reports the expected announcer generation and finisher
   count.
5. Power-loss lanes (T6.1): both the receiver *and* the forwarder are, in
   separate full-stack runs, SIGKILLed mid-stream (after ``0 < count_at_kill <
   NUM_READS`` durable progress) and restarted with the same config/seed/port.
   Each restarted stack must resume losslessly with no duplicate
   ``received_events``/DBF rows. The receiver lane recovers via forwarder
   journal+P2P replay; the forwarder lane recovers via its own durable journal
   while the emulator pauses (``--pause-when-unsubscribed``) so no read is lost
   while the forwarder is down.

Usage::

    uv run scripts/e2e/run_stack.py            # build (if needed) + run connections + BOTH lanes
    uv run scripts/e2e/run_stack.py --no-build # skip cargo build
    uv run scripts/e2e/run_stack.py --keep      # keep the temp dir on exit
    uv run scripts/e2e/run_stack.py --power-loss-target forwarder  # one lane only

Stdlib only; uses ``cargo`` to build the four service binaries. The receiver
binary is built with its loopback-only ``test-bridge`` feature because the
connections / remote-config assertions drive canonical control commands through
``POST /bridge/invoke/{cmd}``.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import re
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TARGET_DIR = REPO_ROOT / "target" / "debug"

# Deterministic 32-byte secret-key seeds (hex). Distinct so the two endpoints
# get distinct endpoint ids.
FORWARDER_SEED_HEX = "cd" * 32
RECEIVER_SEED_HEX = "ab" * 32

# Enrollment vouchers (admin-issued, single-use) the devices present once to
# /register to mint their per-device tokens. Opaque secrets; any string works.
FORWARDER_VOUCHER = "rte-e2e-forwarder-voucher"
RECEIVER_VOUCHER = "rte-e2e-receiver-voucher"

# Deterministic scenario shape.
NUM_READS = 8
READ_DELAY_MS = 200  # spacing between emulator reads; widens the kill window
RECONCILE_MS = 200

FRAME_LEN = 36  # IPICO raw frame length (chars)
WIRE_FRAME = FRAME_LEN + 2  # frame + trailing CRLF as stored/replayed

# Power-loss lanes exercised by the real-process suite. Both are SIGKILL +
# restart of a real OS process mid-stream; the stack must resume losslessly.
POWER_LOSS_TARGETS = ("receiver", "forwarder")


def resolve_power_loss_targets(value: str) -> list[str]:
    """Map a ``--power-loss-target`` value to the ordered list of lanes to run.

    ``"both"`` (the default) enumerates every lane in :data:`POWER_LOSS_TARGETS`
    so the one-command run exercises receiver *and* forwarder SIGKILL. A single
    target name selects just that lane (used by CI sharding / debugging).
    """
    if value == "both":
        return list(POWER_LOSS_TARGETS)
    if value in POWER_LOSS_TARGETS:
        return [value]
    raise ValueError(
        f"unknown power-loss target {value!r}; "
        f"expected 'both' or one of {POWER_LOSS_TARGETS}"
    )


# ---------------------------------------------------------------------------
# Deterministic frame construction
# ---------------------------------------------------------------------------
def build_frame(chip_index: int) -> str:
    """Return a valid 36-char IPICO RAW frame with a distinct tag id.

    Layout (chars): ``aa`` + ``40`` + tag(12) + ``0a2a`` + YY MM DD hh mm ss(12)
    + centis(2) + checksum(2). The checksum is the low byte of the sum of the
    ASCII codes of chars ``[2:34]`` (matching ``ipico_core::read``).
    """
    tag = format(chip_index, "012x")
    core = "aa" + "40" + tag + "0a2a" + "01" + "12" + "30" + "18" + "45" + "59" + "00"
    checksum = sum(ord(c) for c in core[2:34]) % 256
    return core + format(checksum, "02x")


EXPECTED_FRAMES = [build_frame(i) for i in range(1, NUM_READS + 1)]
EXPECTED_TAGS = [f[4:16] for f in EXPECTED_FRAMES]
# As stored in the journal / received_events / replayed by the proxy: with CRLF.
EXPECTED_WIRE = [(f + "\r\n").encode("ascii") for f in EXPECTED_FRAMES]


# ---------------------------------------------------------------------------
# Small utilities
# ---------------------------------------------------------------------------
def free_tcp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def free_udp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def wait_until(predicate, timeout: float, interval: float = 0.05, what: str = "condition"):
    deadline = time.monotonic() + timeout
    last_exc = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                return value
        except Exception as exc:  # noqa: BLE001 - surfaced on timeout
            last_exc = exc
        time.sleep(interval)
    suffix = f" (last error: {last_exc})" if last_exc else ""
    raise TimeoutError(f"timed out waiting for {what}{suffix}")


def wait_for_log(path: Path, needle: str, timeout: float, what: str):
    def _check():
        if not path.exists():
            return False
        return needle in path.read_text(errors="replace")

    wait_until(_check, timeout=timeout, what=what)


# ---------------------------------------------------------------------------
# Process management
# ---------------------------------------------------------------------------
@dataclass
class Managed:
    name: str
    argv: list[str]
    log_path: Path
    env: dict | None = None
    proc: subprocess.Popen | None = None
    log_fh: object | None = None

    def start(self):
        self.log_fh = open(self.log_path, "w")  # noqa: SIM115 - closed in stop()
        env = dict(os.environ)
        if self.env:
            env.update(self.env)
        self.proc = subprocess.Popen(
            self.argv,
            stdout=self.log_fh,
            stderr=subprocess.STDOUT,
            cwd=str(REPO_ROOT),
            env=env,
        )

    def sigkill(self):
        if self.proc is None:
            raise RuntimeError(f"{self.name} was never started")
        if self.proc.poll() is not None:
            raise RuntimeError(
                f"{self.name} exited before SIGKILL with code {self.proc.returncode}; "
                f"see {self.log_path}"
            )
        self.proc.kill()
        self.proc.wait(timeout=10)
        if self.proc.returncode != -signal.SIGKILL:
            raise RuntimeError(
                f"{self.name} did not terminate via SIGKILL; "
                f"returncode={self.proc.returncode}; see {self.log_path}"
            )

    def stop(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait(timeout=8)
        if self.log_fh:
            with contextlib.suppress(Exception):
                self.log_fh.close()
            self.log_fh = None

    def assert_alive(self):
        if self.proc and self.proc.poll() is not None:
            raise RuntimeError(
                f"{self.name} exited early with code {self.proc.returncode}; "
                f"see {self.log_path}"
            )


class Stack:
    def __init__(self):
        self.procs: list[Managed] = []

    def add(self, managed: Managed) -> Managed:
        self.procs.append(managed)
        return managed

    def shutdown(self):
        for managed in reversed(self.procs):
            managed.stop()


# ---------------------------------------------------------------------------
# Build + endpoint-id derivation
# ---------------------------------------------------------------------------
def cargo_build(*, agent_ui: bool = False, lcd_sim: bool = False):
    del agent_ui  # The bridge is now required by first-class E2E assertions.
    print("[build] cargo build -p emulator -p forwarder -p server ...")
    subprocess.run(
        ["cargo", "build", "-p", "emulator", "-p", "forwarder", "-p", "server"],
        cwd=str(REPO_ROOT),
        check=True,
    )
    print("[build] cargo build -p receiver --features test-bridge ...")
    subprocess.run(
        ["cargo", "build", "-p", "receiver", "--features", "test-bridge"],
        cwd=str(REPO_ROOT),
        check=True,
    )
    if lcd_sim:
        print("[build] cargo build -p rt-screen --example lcd_sim --features simulator ...")
        subprocess.run(
            [
                "cargo", "build", "-p", "rt-screen", "--example", "lcd_sim",
                "--features", "simulator",
            ],
            cwd=str(REPO_ROOT),
            check=True,
        )


def bin_path(name: str) -> Path:
    path = TARGET_DIR / name
    if not path.exists():
        raise FileNotFoundError(f"missing built binary {path}; run without --no-build")
    return path


def derive_endpoint_id(seed_hex: str) -> str:
    out = subprocess.run(
        [str(bin_path("receiver-headless")), "print-endpoint-id",
         "--p2p-secret-key-seed-hex", seed_hex],
        cwd=str(REPO_ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    endpoint_id = out.stdout.strip()
    if len(endpoint_id) != 64:
        raise RuntimeError(f"unexpected endpoint id for seed {seed_hex}: {endpoint_id!r}")
    return endpoint_id


# ---------------------------------------------------------------------------
# Receiver DB preseed (canonical stream subscription + DBF profile)
# ---------------------------------------------------------------------------
# Must stay in column-sync with services/receiver/src/storage/schema.sql: the
# receiver refuses to open a DB with tables but user_version == 0, and it no
# longer runs add-column migrations, so the preseeded tables must already
# carry every column the receiver reads.
PRESEED_SQL = """
PRAGMA user_version = 1;
CREATE TABLE IF NOT EXISTS profile (
    server_url  TEXT NOT NULL,
    token       TEXT NOT NULL,
    update_mode TEXT NOT NULL DEFAULT 'check-and-download',
    receiver_mode_json TEXT,
    receiver_id TEXT,
    dbf_enabled INTEGER NOT NULL DEFAULT 0,
    announcer_enabled INTEGER NOT NULL DEFAULT 0,
    announcer_max_list_size INTEGER NOT NULL DEFAULT 25,
    device_token TEXT,
    rd_import_enabled INTEGER NOT NULL DEFAULT 0,
    rd_import_dir TEXT NOT NULL DEFAULT 'C:\\Winrace\\Files',
    rd_import_interval_secs INTEGER NOT NULL DEFAULT 15,
    dbf_flush_interval_ms INTEGER NOT NULL DEFAULT 1000
);
CREATE TABLE IF NOT EXISTS subscriptions (
    forwarder_endpoint_id TEXT NOT NULL,
    stream_id             TEXT NOT NULL,
    local_port_override   INTEGER,
    event_type            TEXT NOT NULL DEFAULT 'finish',
    forwarder_id          TEXT,
    reader_ip             TEXT,
    -- Persisted DBF READER digit (0..=9). Mirrors
    -- services/receiver/src/storage/schema.sql: the receiver only runs
    -- CREATE TABLE IF NOT EXISTS, so the preseeded table must already carry
    -- this column and seeded streams need a concrete digit (NULL is skipped
    -- by DBF delivery).
    dbf_reader_index      INTEGER,
    PRIMARY KEY (forwarder_endpoint_id, stream_id)
);
CREATE TABLE IF NOT EXISTS announcer_publish_streams (
    stream_id TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS participants (
    bib         INTEGER PRIMARY KEY,
    last        TEXT NOT NULL,
    first       TEXT NOT NULL,
    affiliation TEXT NOT NULL,
    gender      TEXT NOT NULL,
    division    INTEGER
);
CREATE TABLE IF NOT EXISTS bib_chips (
    chip_id TEXT PRIMARY KEY,
    bib     INTEGER NOT NULL
);
"""


def local_stream_key(forwarder_endpoint_id: str, stream_id: str) -> str:
    """Receiver-local canonical stream key: `{endpoint_id}\\x1f{wire_stream_id}`.

    Mirrors `services/receiver/src/stream_key.rs`. Durable receiver state
    (received_events, cursors, announcer opt-in, ...) is keyed by this encoded
    form; the wire stream id alone is ambiguous across forwarders.
    """
    return f"{forwarder_endpoint_id}\x1f{stream_id}"


def preseed_receiver_db(db_path: Path, forwarder_endpoint_id: str, stream_id: str,
                        proxy_port: int, dbf_path: Path, *,
                        server_url: str = "p2p://loopback",
                        server_token: str = "e2e-token"):
    conn = sqlite3.connect(str(db_path))
    try:
        conn.executescript(PRESEED_SQL)
        # The receiver writes IPICO.DBF into the Race Director working
        # directory (profile.rd_import_dir).
        conn.execute(
            "INSERT INTO profile "
            "(server_url, token, update_mode, receiver_mode_json, receiver_id, "
            " dbf_enabled, announcer_enabled, rd_import_dir) "
            "VALUES (?,?,?,?,?,?,?,?)",
            (server_url, server_token, "check-and-download", None,
             "rx-e2e", 1, 1, str(dbf_path.parent)),
        )
        conn.execute(
            "INSERT INTO subscriptions "
            "(forwarder_endpoint_id, stream_id, local_port_override, event_type, "
            " forwarder_id, reader_ip, dbf_reader_index) VALUES (?,?,?,?,?,?,?)",
            (forwarder_endpoint_id, stream_id, proxy_port, "finish", None, stream_id, 0),
        )
        # Opt the seeded stream in to announcer publishing (opt-in default).
        # Keyed by the canonical local stream key, not the bare wire id.
        conn.execute(
            "INSERT INTO announcer_publish_streams (stream_id) VALUES (?)",
            (local_stream_key(forwarder_endpoint_id, stream_id),),
        )
        # Seed participant + chip data so announcer rows carry bib/name. Each
        # emulated chip tag (EXPECTED_TAGS, bibs 1..NUM_READS) maps to a named
        # participant.
        for i, tag in enumerate(EXPECTED_TAGS, start=1):
            conn.execute(
                "INSERT INTO participants (bib, last, first, affiliation, gender) "
                "VALUES (?,?,?,?,?)",
                (i, f"Last{i}", f"First{i}", "", "X"),
            )
            conn.execute(
                "INSERT INTO bib_chips (chip_id, bib) VALUES (?,?)",
                (tag, i),
            )
        conn.commit()
    finally:
        conn.close()


# ---------------------------------------------------------------------------
# Receiver SQLite reads
# ---------------------------------------------------------------------------
def load_received_events(db_path: Path) -> list[dict]:
    if not db_path.exists():
        return []
    conn = sqlite3.connect(f"file:{db_path}?mode=ro", uri=True, timeout=5)
    try:
        conn.row_factory = sqlite3.Row
        rows = conn.execute(
            "SELECT stream_id, seq, epoch, raw_frame, read_kind "
            "FROM received_events ORDER BY seq"
        ).fetchall()
        return [dict(r) for r in rows]
    except sqlite3.OperationalError as exc:
        if "no such table: received_events" in str(exc):
            return []
        raise
    finally:
        conn.close()


def received_count(db_path: Path) -> int:
    return len(load_received_events(db_path))


# ---------------------------------------------------------------------------
# Forwarder journal progress (durable events the forwarder has committed)
# ---------------------------------------------------------------------------
def forwarder_event_count(journal_path: Path, stream_id: str) -> int:
    """Return the number of durably-journaled forwarder events for a stream.

    Reads the forwarder's SQLite journal ``events`` table read-only. Used by the
    forwarder power-loss lane to confirm ``0 < count_at_kill < NUM_READS``
    durable progress before issuing SIGKILL.
    """
    if not journal_path.exists():
        return 0
    conn = sqlite3.connect(f"file:{journal_path}?mode=ro", uri=True, timeout=5)
    try:
        row = conn.execute(
            "SELECT COUNT(*) FROM events WHERE stream_id = ?", (stream_id,)
        ).fetchone()
        return int(row[0]) if row else 0
    except sqlite3.OperationalError as exc:
        if "no such table: events" in str(exc):
            return 0
        raise
    finally:
        conn.close()


def partial_forwarder_count(journal_path: Path, stream_id: str):
    count = forwarder_event_count(journal_path, stream_id)
    return count if 0 < count < NUM_READS else False


# ---------------------------------------------------------------------------
# DBF record count (Visual FoxPro header: record count at bytes 4..8 LE)
# ---------------------------------------------------------------------------
def dbf_record_count(dbf_path: Path) -> int:
    data = dbf_path.read_bytes()
    if len(data) < 32:
        raise AssertionError(f"DBF file too small ({len(data)} bytes)")
    return int.from_bytes(data[4:8], "little")


def dbf_records(dbf_path: Path) -> list[dict[str, str]]:
    data = dbf_path.read_bytes()
    if len(data) < 32:
        raise AssertionError(f"DBF file too small ({len(data)} bytes)")
    record_count = int.from_bytes(data[4:8], "little")
    header_size = int.from_bytes(data[8:10], "little")
    record_size = int.from_bytes(data[10:12], "little")
    if record_size < 41:
        raise AssertionError(f"unexpected DBF record size {record_size}")
    records = []
    for i in range(record_count):
        start = header_size + i * record_size
        end = start + record_size
        record = data[start:end]
        if len(record) != record_size:
            raise AssertionError(f"truncated DBF record {i}: {len(record)} bytes")
        if record[0] != 0x20:
            continue  # deleted/tombstoned row
        records.append({
            "event": record[1:2].decode("ascii").strip(),
            "chip": record[4:16].decode("ascii").strip(),
            "reader": record[40:41].decode("ascii").strip(),
        })
    return records


# ---------------------------------------------------------------------------
# TCP proxy replay
# ---------------------------------------------------------------------------
def read_proxy_replay(port: int, expected_bytes: int, timeout: float = 10.0) -> bytes:
    deadline = time.monotonic() + timeout
    buf = bytearray()
    with socket.create_connection(("127.0.0.1", port), timeout=timeout) as sock:
        sock.settimeout(0.5)
        while len(buf) < expected_bytes and time.monotonic() < deadline:
            try:
                chunk = sock.recv(4096)
            except socket.timeout:
                continue
            if not chunk:
                break
            buf.extend(chunk)
    return bytes(buf)


# ---------------------------------------------------------------------------
# Server HTTP + receiver bridge HTTP
# ---------------------------------------------------------------------------
def server_status(base_url: str) -> dict:
    with urllib.request.urlopen(f"{base_url}/status", timeout=5) as resp:
        return json.loads(resp.read().decode())


def server_healthy(base_url: str) -> bool:
    try:
        with urllib.request.urlopen(f"{base_url}/healthz", timeout=2) as resp:
            return resp.status == 200
    except Exception:  # noqa: BLE001
        return False


def post_json(url: str, body: dict, *, headers: dict[str, str] | None = None) -> dict:
    data = json.dumps(body).encode("utf-8")
    request_headers = {"Content-Type": "application/json"}
    if headers:
        request_headers.update(headers)
    request = urllib.request.Request(url, data=data, headers=request_headers, method="POST")
    with urllib.request.urlopen(request, timeout=5) as resp:
        payload = resp.read().decode()
    return json.loads(payload) if payload else {}


def server_approve_device(base_url: str, endpoint_id: str) -> dict:
    return post_json(
        f"{base_url}/admin/devices/approve",
        {"endpoint_id": endpoint_id},
        headers={"Remote-User": "rt-e2e-admin"},
    )


def server_create_enrollment_token(base_url: str, device_kind: str, token: str) -> dict:
    """Create an enrollment voucher with a fixed secret via the admin route."""
    return post_json(
        f"{base_url}/admin/enrollment-tokens",
        {"device_kind": device_kind, "token": token, "display_name": f"e2e {device_kind}"},
        headers={"Remote-User": "rt-e2e-admin"},
    )


def bridge_invoke(base_url: str, cmd: str, args: dict | None = None) -> dict:
    return post_json(f"{base_url.rstrip('/')}/bridge/invoke/{cmd}", args or {})


def bridge_invoke_error(base_url: str, cmd: str, args: dict | None = None) -> tuple[int, dict]:
    try:
        bridge_invoke(base_url, cmd, args)
    except urllib.error.HTTPError as exc:
        payload = exc.read().decode()
        body = json.loads(payload) if payload else {}
        return exc.code, body
    raise AssertionError(f"expected bridge command {cmd!r} to fail")


def forwarder_connection(connections: dict, endpoint_id: str) -> dict | None:
    for forwarder in connections.get("forwarders", []):
        if forwarder.get("endpoint_id") == endpoint_id:
            return forwarder
    return None


def server_device_approval(base_url: str, endpoint_id: str) -> str | None:
    for device in server_status(base_url).get("devices", []):
        if device.get("endpoint_id") == endpoint_id:
            return device.get("approval_state")
    return None


# ---------------------------------------------------------------------------
# Optional exploratory bridge-agent artifact capture (T5.5)
# ---------------------------------------------------------------------------
def receiver_headless_url(log_path: Path) -> str:
    prefix = "receiver-headless listening on "
    for line in reversed(log_path.read_text(errors="replace").splitlines()):
        idx = line.find(prefix)
        if idx >= 0:
            return line[idx + len(prefix):].strip()
    raise RuntimeError(f"receiver headless URL not found in {log_path}")


def emit_agent_ui_artifacts(
    *,
    receiver_log_path: Path,
    scenario_path: Path,
    artifacts_dir: Path,
    expected_stream_id: str,
    expected_local_port: int,
):
    try:
        if str(REPO_ROOT) not in sys.path:
            sys.path.insert(0, str(REPO_ROOT))
        from scripts.e2e.agent_ui.bridge_agent import run_bridge_goal

        bridge_url = receiver_headless_url(receiver_log_path)
        findings = run_bridge_goal(
            bridge_base_url=bridge_url,
            scenario_path=scenario_path,
            artifacts_dir=artifacts_dir,
            expected_stream_id=expected_stream_id,
            expected_local_port=expected_local_port,
        )
        print(f"[agent-ui] artifacts written to {artifacts_dir}")
        print(f"[agent-ui] completed={findings.get('completed')} passed={findings.get('passed')}")
    except Exception as exc:  # noqa: BLE001 - advisory layer must not gate backend checks
        print(f"[agent-ui] artifact capture failed: {type(exc).__name__}: {exc}", file=sys.stderr)


# ---------------------------------------------------------------------------
# Assertion bookkeeping
# ---------------------------------------------------------------------------
@dataclass
class Results:
    checks: list[tuple[str, bool, str]] = field(default_factory=list)

    def check(self, name: str, ok: bool, detail: str = ""):
        self.checks.append((name, ok, detail))
        status = "PASS" if ok else "FAIL"
        line = f"  [{status}] {name}"
        if detail:
            line += f" — {detail}"
        print(line)

    def expect_eq(self, name: str, actual, expected):
        ok = actual == expected
        detail = "" if ok else f"expected {expected!r}, got {actual!r}"
        self.check(name, ok, detail)

    @property
    def all_passed(self) -> bool:
        return all(ok for _, ok, _ in self.checks)


# ---------------------------------------------------------------------------
# Orchestration
# ---------------------------------------------------------------------------
def assert_received_events(results: Results, db_path: Path, label: str,
                           expected_stream_key: str):
    events = load_received_events(db_path)
    results.expect_eq(f"{label}: received_events count == {NUM_READS}",
                      len(events), NUM_READS)
    seqs = [e["seq"] for e in events]
    results.expect_eq(f"{label}: seqs are exactly 1..{NUM_READS}",
                      seqs, list(range(1, NUM_READS + 1)))
    results.expect_eq(f"{label}: no duplicate seqs", len(set(seqs)), len(seqs))
    raw = [bytes(e["raw_frame"]) for e in events]
    results.expect_eq(f"{label}: raw frames match deterministic scenario",
                      raw, EXPECTED_WIRE)
    epochs = {e["epoch"] for e in events}
    results.expect_eq(f"{label}: single stream epoch", epochs, {1})
    kinds = {e["read_kind"] for e in events}
    results.expect_eq(f"{label}: read_kind is 'raw'", kinds, {"raw"})
    stream_ids = {e["stream_id"] for e in events}
    results.expect_eq(f"{label}: canonical stream key", stream_ids, {expected_stream_key})


def partial_received_count(db_path: Path):
    count = received_count(db_path)
    return count if 0 < count < NUM_READS else False


def dbf_ready(dbf_path: Path) -> bool:
    return dbf_record_count(dbf_path) == NUM_READS


def server_announcer_ready(base_url: str):
    status = server_status(base_url)
    pushed_chips = {row.get("chip_id") for row in status.get("announcer_rows", [])}
    if (status.get("announcer_source_generation", 0) >= 1
            and status.get("finisher_count") == NUM_READS
            and set(EXPECTED_TAGS).issubset(pushed_chips)):
        return status
    return False


def run_connection_scenarios(
    tmp: Path,
    results: Results,
    stack: Stack,
    *,
    lcd_sim: bool = False,
):
    print("\n########## connections + remote-config scenarios ##########")

    # --- Ports (loopback only) ---
    emulator_port = free_tcp_port()
    forwarder_status_port = free_tcp_port()
    forwarder_fanout_port = free_tcp_port()
    forwarder_p2p_port = free_udp_port()
    server_port = free_tcp_port()
    proxy_port = free_tcp_port()

    stream_id = f"127.0.0.1:{emulator_port}"
    server_url = f"http://127.0.0.1:{server_port}"

    # --- Deterministic endpoint ids (shared seed->id derivation) ---
    forwarder_endpoint_id = derive_endpoint_id(FORWARDER_SEED_HEX)
    receiver_endpoint_id = derive_endpoint_id(RECEIVER_SEED_HEX)
    print(f"[ids] forwarder={forwarder_endpoint_id[:16]}…  receiver={receiver_endpoint_id[:16]}…")

    # --- Files ---
    reads_file = tmp / "reads.txt"
    reads_file.write_text("\n".join(EXPECTED_FRAMES) + "\n")

    token_file = tmp / "forwarder-token"
    token_file.write_text("e2e-forwarder-token\n")
    # Bootstrap voucher the forwarder presents to /register to mint its token.
    server_token_file = tmp / "server-token"
    server_token_file.write_text(f"{FORWARDER_VOUCHER}\n")
    forwarder_device_token_file = tmp / "forwarder-device-token"

    journal_path = tmp / "forwarder.sqlite3"
    thin_db_path = tmp / "server.sqlite3"
    receiver_data_dir = tmp / "receiver-data"
    receiver_data_dir.mkdir(parents=True, exist_ok=True)
    receiver_db_path = receiver_data_dir / "receiver.sqlite3"
    dbf_path = tmp / "IPICO.DBF"

    forwarder_config = tmp / "forwarder.toml"
    forwarder_config.write_text(
        f"""schema_version = 1

display_name = "E2E Forwarder"

[auth]
token_file = "{token_file}"

[journal]
sqlite_path = "{journal_path}"

[status_http]
bind = "127.0.0.1:{forwarder_status_port}"

[control]
allow_remote_config = true

[[readers]]
target = "127.0.0.1:{emulator_port}"
enabled = true
local_fallback_port = {forwarder_fanout_port}

[p2p]
enabled = true
secret_key_seed_hex = "{FORWARDER_SEED_HEX}"
bind_addr_v4 = "127.0.0.1:{forwarder_p2p_port}"
relay_disabled = true
discovery_disabled = true
max_concurrent_bidi_streams = 256
server_url = "{server_url}"
server_token_file = "{server_token_file}"
device_token_file = "{forwarder_device_token_file}"
allowlist_poll_interval_secs = 1
allowlist_request_timeout_secs = 2
"""
    )

    # --- Preseed receiver DB (canonical subscription + DBF profile) ---
    preseed_receiver_db(
        receiver_db_path,
        forwarder_endpoint_id,
        stream_id,
        proxy_port,
        dbf_path,
        server_url=server_url,
        server_token=RECEIVER_VOUCHER,
    )

    # --- 1. server (trusted proxy enabled only for loopback admin approval) ---
    thin = stack.add(Managed(
        name="server[connections]",
        argv=[str(bin_path("server"))],
        log_path=tmp / "server.log",
        env={
            "SERVER_DB_PATH": str(thin_db_path),
            "BIND_ADDR": f"127.0.0.1:{server_port}",
            "SERVER_TRUSTED_PROXY": "1",
            "LOG_LEVEL": "info",
        },
    ))
    thin.start()
    wait_until(lambda: server_healthy(server_url), timeout=20,
               what="server /healthz")
    print("[up] server healthy")

    # Issue the single-use enrollment vouchers the devices bootstrap from.
    server_create_enrollment_token(server_url, "forwarder", FORWARDER_VOUCHER)
    server_create_enrollment_token(server_url, "receiver", RECEIVER_VOUCHER)
    print("[up] enrollment vouchers issued")

    # --- 2. emulator (deterministic, verbatim, once) ---
    emulator = stack.add(Managed(
        name="emulator[connections]",
        argv=[str(bin_path("emulator")),
              "-p", str(emulator_port),
              "-f", str(reads_file),
              "-d", str(READ_DELAY_MS),
              "-t", "raw",
              "--verbatim", "--once"],
        log_path=tmp / "emulator.log",
        env={"RUST_LOG": "info"},
    ))
    emulator.start()
    wait_for_log(emulator.log_path, "[emulator] listening on", timeout=20,
                 what="emulator TCP listener")
    emulator.assert_alive()
    print("[up] emulator listening")

    # --- 3. forwarder (server-distributed allow-list starts empty/pending) ---
    def make_forwarder(suffix: str) -> Managed:
        return Managed(
            name=f"forwarder[connections-{suffix}]",
            argv=[str(bin_path("forwarder")), "--config", str(forwarder_config)],
            log_path=tmp / f"forwarder-{suffix}.log",
            env={"RUST_LOG": "info,forwarder=debug"},
        )

    forwarder = stack.add(make_forwarder("1"))
    forwarder.start()
    wait_for_log(forwarder.log_path, "p2p iroh server started", timeout=30,
                 what="forwarder p2p startup")
    forwarder.assert_alive()
    print("[up] forwarder p2p serving")

    # Approve the forwarder so it may fetch the receiver allow-list (an active
    # forwarder is required; the receiver is approved later in the scenario).
    wait_until(lambda: server_device_approval(server_url, forwarder_endpoint_id) is not None,
               timeout=15, what="forwarder self-registration")
    server_approve_device(server_url, forwarder_endpoint_id)
    print("[up] forwarder approved")

    # --- 4. receiver-headless (bridge + P2P + server announcer) ---
    receiver = stack.add(Managed(
        name="receiver-headless[connections]",
        argv=[
            str(bin_path("receiver-headless")),
            "--data-dir", str(receiver_data_dir),
            "--bind-addr", "127.0.0.1:0",
            "--receiver-id", "rx-e2e",
            "--p2p-forwarder-endpoint-id", forwarder_endpoint_id,
            "--p2p-forwarder-direct-addr", f"127.0.0.1:{forwarder_p2p_port}",
            "--p2p-secret-key-seed-hex", RECEIVER_SEED_HEX,
            "--p2p-server-url", server_url,
            "--p2p-server-token", RECEIVER_VOUCHER,
            "--p2p-reconcile-ms", str(RECONCILE_MS),
        ],
        log_path=tmp / "receiver.log",
        env={"RUST_LOG": "info,receiver=debug"},
    ))
    receiver.start()
    wait_for_log(receiver.log_path, "receiver-headless listening on", timeout=30,
                 what="receiver bridge startup")
    bridge_url = receiver_headless_url(receiver.log_path)
    print(f"[up] receiver-headless bridge at {bridge_url}")

    def pending_receiver_connections():
        connections = bridge_invoke(bridge_url, "get_connections")
        server = connections.get("server", {})
        forwarder_status = forwarder_connection(connections, forwarder_endpoint_id)
        if (server.get("approval_state") == "pending"
                and server_device_approval(server_url, receiver_endpoint_id) == "pending"
                and forwarder_status is not None
                and forwarder_status.get("state") != "subscribed"):
            return connections
        return False

    pending_connections = wait_until(
        pending_receiver_connections,
        timeout=30,
        what="receiver to be pending approval and not subscribed",
    )
    pending_forwarder = forwarder_connection(pending_connections, forwarder_endpoint_id) or {}
    results.expect_eq("connections: receiver starts pending server approval",
                      pending_connections.get("server", {}).get("approval_state"), "pending")
    results.expect_eq("connections: server status has receiver pending",
                      server_device_approval(server_url, receiver_endpoint_id), "pending")
    results.check("connections: unapproved receiver is not subscribed",
                  pending_forwarder.get("state") != "subscribed",
                  f"state={pending_forwarder.get('state')} pending={pending_forwarder.get('pending')}")

    approved = server_approve_device(server_url, receiver_endpoint_id)
    results.expect_eq("connections: admin approval returns active receiver",
                      approved.get("approval_state"), "active")

    def subscribed_after_approval():
        connections = bridge_invoke(bridge_url, "get_connections")
        server = connections.get("server", {})
        forwarder_status = forwarder_connection(connections, forwarder_endpoint_id)
        if (server.get("approval_state") == "active"
                and server_device_approval(server_url, receiver_endpoint_id) == "active"
                and forwarder_status is not None
                and forwarder_status.get("state") == "subscribed"
                and forwarder_status.get("subscribed_count") == 1
                and forwarder_status.get("remote_config_available") is True):
            return connections
        return False

    active_connections = wait_until(
        subscribed_after_approval,
        timeout=45,
        what="receiver to auto-connect after server approval",
    )
    active_forwarder = forwarder_connection(active_connections, forwarder_endpoint_id) or {}
    results.expect_eq("connections: receiver server approval is active",
                      active_connections.get("server", {}).get("approval_state"), "active")
    results.expect_eq("connections: approved receiver auto-connects/subscribes",
                      active_forwarder.get("state"), "subscribed")
    results.expect_eq("connections: remote config capability is advertised",
                      active_forwarder.get("remote_config_available"), True)

    wait_until(lambda: received_count(receiver_db_path) >= NUM_READS, timeout=45,
               what=f"receiver to persist all {NUM_READS} events")
    wait_until(lambda: dbf_ready(dbf_path), timeout=20,
               what=f"DBF to contain all {NUM_READS} rows")
    status = wait_until(lambda: server_announcer_ready(server_url), timeout=20,
                        what="server announcer to receive all rows")
    assert_received_events(results, receiver_db_path,
                           "received_events (approval auto-connect)",
                           local_stream_key(forwarder_endpoint_id, stream_id))
    results.expect_eq(f"connections: server finisher_count == {NUM_READS}",
                      status.get("finisher_count"), NUM_READS)

    if lcd_sim:
        lcd_sim_bin = TARGET_DIR / "examples" / "lcd_sim"
        if not lcd_sim_bin.exists():
            raise FileNotFoundError(
                f"missing built example {lcd_sim_bin}; run without --no-build or build it first"
            )
        png_path = tmp / "lcd-status.png"
        env = {**os.environ, "EG_SIMULATOR_DUMP_RAW": str(png_path)}
        proc = subprocess.run(
            [
                str(lcd_sim_bin),
                "--once",
                "--require-live",
                "--url",
                f"http://127.0.0.1:{forwarder_status_port}",
            ],
            cwd=str(REPO_ROOT),
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )
        ok = proc.returncode == 0 and png_path.exists() and png_path.stat().st_size > 0
        if not ok:
            print(
                f"[lcd-sim] rc={proc.returncode} "
                f"stdout={proc.stdout!r} stderr={proc.stderr!r}"
            )
        results.check(
            "lcd-sim: PNG rendered from live /api/v1/display-state (exit 0 + non-empty PNG)",
            ok,
        )

    bridge_invoke(bridge_url, "disconnect_forwarder", {"endpoint_id": forwarder_endpoint_id})

    def forwarder_disconnected():
        connections = bridge_invoke(bridge_url, "get_connections")
        forwarder_status = forwarder_connection(connections, forwarder_endpoint_id)
        if (forwarder_status is not None
                and forwarder_status.get("state") == "disconnected"
                and forwarder_status.get("pending") is False
                and forwarder_status.get("remote_config_available") is False):
            return forwarder_status
        return False

    disconnected = wait_until(
        forwarder_disconnected,
        timeout=20,
        what="forwarder to disconnect via per-forwarder control",
    )
    results.expect_eq("connections: disconnect_forwarder stops forwarder worker",
                      disconnected.get("state"), "disconnected")
    results.expect_eq("connections: disconnected forwarder drops remote config session",
                      disconnected.get("remote_config_available"), False)

    bridge_invoke(bridge_url, "connect_forwarder", {"endpoint_id": forwarder_endpoint_id})
    reconnected = wait_until(
        subscribed_after_approval,
        timeout=45,
        what="forwarder to reconnect via per-forwarder control",
    )
    reconnected_forwarder = forwarder_connection(reconnected, forwarder_endpoint_id) or {}
    results.expect_eq("connections: connect_forwarder restores subscription",
                      reconnected_forwarder.get("state"), "subscribed")

    config_response = bridge_invoke(
        bridge_url,
        "get_forwarder_config",
        {"endpoint_id": forwarder_endpoint_id},
    )
    config_json = config_response.get("config_json")
    results.check("remote config: get returns non-empty config_json",
                  isinstance(config_json, str) and len(config_json) > 0)
    config_doc = json.loads(config_json)
    results.expect_eq("remote config: get returns schema_version 1",
                      config_doc.get("schema_version"), 1)

    config_doc["display_name"] = "E2E Remote Config Updated"
    set_response = bridge_invoke(
        bridge_url,
        "set_forwarder_config",
        {"endpoint_id": forwarder_endpoint_id, "config_json": json.dumps(config_doc)},
    )
    results.expect_eq("remote config: set full document succeeds",
                      set_response.get("ok"), True)
    results.expect_eq("remote config: set reports restart_needed",
                      set_response.get("restart_needed"), True)

    updated_config_response = bridge_invoke(
        bridge_url,
        "get_forwarder_config",
        {"endpoint_id": forwarder_endpoint_id},
    )
    updated_doc = json.loads(updated_config_response.get("config_json", "{}"))
    results.expect_eq("remote config: updated document is readable",
                      updated_doc.get("display_name"), "E2E Remote Config Updated")

    # [control] is a protected section: a remote attempt to flip
    # allow_remote_config must be rejected and must not persist.
    protected_doc = json.loads(json.dumps(updated_doc))
    protected_doc.setdefault("control", {})["allow_remote_config"] = False
    disable_response = bridge_invoke(
        bridge_url,
        "set_forwarder_config",
        {"endpoint_id": forwarder_endpoint_id, "config_json": json.dumps(protected_doc)},
    )
    results.expect_eq("remote config protection: remote [control] flip is rejected",
                      disable_response.get("ok"), False)
    results.check("remote config protection: error names the protected section",
                  "control" in (disable_response.get("error") or ""),
                  disable_response.get("error") or "")
    recheck_response = bridge_invoke(
        bridge_url,
        "get_forwarder_config",
        {"endpoint_id": forwarder_endpoint_id},
    )
    recheck_doc = json.loads(recheck_response.get("config_json", "{}"))
    results.expect_eq("remote config protection: allow_remote_config unchanged after rejection",
                      (recheck_doc.get("control") or {}).get("allow_remote_config"), True)

    forwarder.stop()

    # Gate remote config the supported way: a local (trusted) config edit.
    # Remote writes to [control] are rejected above, so the gating scenario
    # flips the flag directly in the forwarder's TOML before the restart.
    # Line-anchored so it tolerates formatting drift, and parse-verified below
    # so a silent non-edit can never produce a confusing downstream failure.
    config_text = forwarder_config.read_text()
    new_text, n_subs = re.subn(
        r"(?m)^(\s*allow_remote_config\s*=\s*)true\s*$",
        r"\g<1>false",
        config_text,
    )
    if n_subs != 1:
        raise AssertionError(
            f"expected exactly one 'allow_remote_config = true' line in forwarder "
            f"config, found {n_subs}:\n{config_text}"
        )
    forwarder_config.write_text(new_text)
    parsed = tomllib.loads(new_text)
    if parsed.get("control", {}).get("allow_remote_config") is not False:
        raise AssertionError(
            f"config edit did not disable remote config:\n{new_text}"
        )
    forwarder2 = stack.add(make_forwarder("2"))
    forwarder2.start()
    wait_for_log(forwarder2.log_path, "p2p iroh server started", timeout=30,
                 what="forwarder restart with remote config disabled")
    forwarder2.assert_alive()
    bridge_invoke(bridge_url, "reconnect_forwarder", {"endpoint_id": forwarder_endpoint_id})

    def remote_config_gated():
        connections = bridge_invoke(bridge_url, "get_connections")
        forwarder_status = forwarder_connection(connections, forwarder_endpoint_id)
        if (forwarder_status is not None
                and forwarder_status.get("state") in {"connected", "subscribed"}
                and forwarder_status.get("remote_config_available") is False):
            return forwarder_status
        return False

    gated_forwarder = wait_until(
        remote_config_gated,
        timeout=45,
        what="forwarder to reconnect without remote-config capability",
    )
    results.expect_eq("remote config gating: get_connections reports unavailable",
                      gated_forwarder.get("remote_config_available"), False)
    error_status, error_body = bridge_invoke_error(
        bridge_url,
        "set_forwarder_config",
        {"endpoint_id": forwarder_endpoint_id, "config_json": json.dumps(updated_doc)},
    )
    results.expect_eq("remote config gating: set is rejected when disabled",
                      error_status, 409)
    results.check("remote config gating: set error mentions unavailable",
                  "remote config unavailable" in error_body.get("error", ""),
                  error_body.get("error", ""))

    return stack


def run(
    tmp: Path,
    results: Results,
    stack: Stack,
    *,
    power_loss_target: str = "receiver",
    agent_ui_scenario: Path | None = None,
    agent_ui_artifacts_dir: Path | None = None,
):
    if power_loss_target not in POWER_LOSS_TARGETS:
        raise ValueError(f"invalid power_loss_target {power_loss_target!r}")
    print(f"\n########## power-loss lane: SIGKILL {power_loss_target} ##########")

    # --- Ports (loopback only) ---
    emulator_port = free_tcp_port()
    forwarder_status_port = free_tcp_port()
    forwarder_fanout_port = free_tcp_port()
    forwarder_p2p_port = free_udp_port()
    server_port = free_tcp_port()
    proxy_port = free_tcp_port()

    stream_id = f"127.0.0.1:{emulator_port}"
    server_url = f"http://127.0.0.1:{server_port}"

    # --- Deterministic endpoint ids (shared seed->id derivation) ---
    forwarder_endpoint_id = derive_endpoint_id(FORWARDER_SEED_HEX)
    receiver_endpoint_id = derive_endpoint_id(RECEIVER_SEED_HEX)
    print(f"[ids] forwarder={forwarder_endpoint_id[:16]}…  receiver={receiver_endpoint_id[:16]}…")

    # --- Files ---
    reads_file = tmp / "reads.txt"
    reads_file.write_text("\n".join(EXPECTED_FRAMES) + "\n")

    token_file = tmp / "forwarder-token"
    token_file.write_text("e2e-forwarder-token\n")

    journal_path = tmp / "forwarder.sqlite3"
    thin_db_path = tmp / "server.sqlite3"
    receiver_data_dir = tmp / "receiver-data"
    receiver_data_dir.mkdir(parents=True, exist_ok=True)
    receiver_db_path = receiver_data_dir / "receiver.sqlite3"
    dbf_path = tmp / "IPICO.DBF"

    forwarder_config = tmp / "forwarder.toml"
    forwarder_config.write_text(
        f"""schema_version = 1

[auth]
token_file = "{token_file}"

[journal]
sqlite_path = "{journal_path}"

[status_http]
bind = "127.0.0.1:{forwarder_status_port}"

[[readers]]
target = "127.0.0.1:{emulator_port}"
enabled = true
local_fallback_port = {forwarder_fanout_port}

[p2p]
enabled = true
secret_key_seed_hex = "{FORWARDER_SEED_HEX}"
bind_addr_v4 = "127.0.0.1:{forwarder_p2p_port}"
relay_disabled = true
discovery_disabled = true
max_concurrent_bidi_streams = 256
static_allowed_receivers = ["{receiver_endpoint_id}"]
"""
    )

    # --- Preseed receiver DB (canonical subscription + DBF profile) ---
    preseed_receiver_db(receiver_db_path, forwarder_endpoint_id, stream_id, proxy_port, dbf_path)

    # --- 1. server ---
    thin = stack.add(Managed(
        name="server",
        argv=[str(bin_path("server"))],
        log_path=tmp / "server.log",
        env={
            "SERVER_DB_PATH": str(thin_db_path),
            "BIND_ADDR": f"127.0.0.1:{server_port}",
            "SERVER_TRUSTED_PROXY": "1",
            "LOG_LEVEL": "info",
        },
    ))
    thin.start()
    wait_until(lambda: server_healthy(server_url), timeout=20,
               what="server /healthz")
    print("[up] server healthy")

    # The receiver bootstraps its device token from this voucher; the forwarder
    # uses a static allow-list in this lane and never contacts the server.
    server_create_enrollment_token(server_url, "receiver", RECEIVER_VOUCHER)
    print("[up] receiver enrollment voucher issued")

    # --- 2. emulator (deterministic, verbatim, once) ---
    emulator_argv = [str(bin_path("emulator")),
                     "-p", str(emulator_port),
                     "-f", str(reads_file),
                     "-d", str(READ_DELAY_MS),
                     "-t", "raw",
                     "--verbatim", "--once"]
    if power_loss_target == "forwarder":
        # The forwarder is the only TCP client of the emulator. When it is
        # SIGKILLed mid-stream the emulator must not race ahead and drop the
        # reads the (restarted) forwarder still needs: pausing while no client
        # is connected makes the forwarder power-loss resume lossless.
        emulator_argv.append("--pause-when-unsubscribed")
    emulator = stack.add(Managed(
        name="emulator",
        argv=emulator_argv,
        log_path=tmp / "emulator.log",
        env={"RUST_LOG": "info"},
    ))
    emulator.start()
    wait_for_log(emulator.log_path, "[emulator] listening on", timeout=20,
                 what="emulator TCP listener")
    emulator.assert_alive()
    print("[up] emulator listening")

    # --- 3. forwarder (P2P, seeded, relay/discovery off, static allow-list) ---
    def make_forwarder(suffix: str) -> Managed:
        return Managed(
            name=f"forwarder[{suffix}]",
            argv=[str(bin_path("forwarder")), "--config", str(forwarder_config)],
            log_path=tmp / f"forwarder-{suffix}.log",
            env={"RUST_LOG": "info,forwarder=debug"},
        )

    forwarder = stack.add(make_forwarder("1"))
    forwarder.start()
    wait_for_log(forwarder.log_path, "p2p iroh server started", timeout=30,
                 what="forwarder p2p startup")
    forwarder.assert_alive()
    print("[up] forwarder p2p serving")

    # --- 4. receiver-headless (P2P + server announcer) ---
    receiver_argv = [
        str(bin_path("receiver-headless")),
        "--data-dir", str(receiver_data_dir),
        "--bind-addr", "127.0.0.1:0",
        "--receiver-id", "rx-e2e",
        "--p2p-forwarder-endpoint-id", forwarder_endpoint_id,
        "--p2p-forwarder-direct-addr", f"127.0.0.1:{forwarder_p2p_port}",
        "--p2p-secret-key-seed-hex", RECEIVER_SEED_HEX,
        # Loopback transport: explicit (a seed already implies this, but state it).
        "--p2p-relay-disabled",
        "--p2p-discovery-disabled",
        "--p2p-server-url", server_url,
        "--p2p-server-token", RECEIVER_VOUCHER,
        "--p2p-reconcile-ms", str(RECONCILE_MS),
    ]

    def make_receiver(suffix: str) -> Managed:
        return Managed(
            name=f"receiver-headless[{suffix}]",
            argv=list(receiver_argv),
            log_path=tmp / f"receiver-{suffix}.log",
            env={"RUST_LOG": "info,receiver=debug"},
        )

    receiver = stack.add(make_receiver("1"))
    receiver.start()
    wait_for_log(receiver.log_path, "p2p_endpoint_id=", timeout=30,
                 what="receiver p2p startup")
    print("[up] receiver-headless p2p running")

    # Approve the receiver so it can take over the announcer generation and push
    # rows (an active receiver is required). The static forwarder allow-list
    # admits it on the data plane regardless; this gates only the server plane.
    wait_until(lambda: server_device_approval(server_url, receiver_endpoint_id) is not None,
               timeout=15, what="receiver self-registration")
    server_approve_device(server_url, receiver_endpoint_id)
    print("[up] receiver approved")

    # --- Power-loss lane: SIGKILL the target mid-stream, then restart ---
    active_receiver = receiver
    if power_loss_target == "receiver":
        print("[power-loss] waiting for first received event, then SIGKILL receiver")
        count_at_kill = wait_until(lambda: partial_received_count(receiver_db_path),
                                   timeout=30,
                                   what="receiver to persist a partial event set")
        receiver.sigkill()
        print(f"[power-loss] SIGKILLed receiver with {count_at_kill}/{NUM_READS} "
              "events persisted")

        receiver2 = stack.add(make_receiver("2"))
        receiver2.start()
        wait_for_log(receiver2.log_path, "p2p_endpoint_id=", timeout=30,
                     what="receiver restart p2p startup")
        active_receiver = receiver2
        print("[up] receiver-headless restarted")
    else:  # power_loss_target == "forwarder"
        print("[power-loss] waiting for durable journal progress, then SIGKILL forwarder")
        count_at_kill = wait_until(
            lambda: partial_forwarder_count(journal_path, stream_id),
            timeout=30,
            what="forwarder to durably journal a partial event set",
        )
        forwarder.sigkill()
        print(f"[power-loss] SIGKILLed forwarder with {count_at_kill}/{NUM_READS} "
              "events durably journaled")

        forwarder2 = stack.add(make_forwarder("2"))
        forwarder2.start()
        wait_for_log(forwarder2.log_path, "p2p iroh server started", timeout=30,
                     what="forwarder restart p2p startup")
        forwarder2.assert_alive()
        print("[up] forwarder restarted; receiver resumes via journal replay")

    # --- Wait for the full deterministic set and async output workers. ---
    wait_until(lambda: received_count(receiver_db_path) >= NUM_READS, timeout=45,
               what=f"receiver to persist all {NUM_READS} events")
    wait_until(lambda: dbf_ready(dbf_path), timeout=20,
               what=f"DBF to contain all {NUM_READS} rows")
    status = wait_until(lambda: server_announcer_ready(server_url), timeout=20,
                        what="server announcer to receive all rows")

    print("\n=== Assertions ===")

    # 1 + 5. Receiver received_events exact + lossless / no-dup after resume.
    assert_received_events(results, receiver_db_path,
                           f"received_events (post-resume, {power_loss_target} kill)",
                           local_stream_key(forwarder_endpoint_id, stream_id))
    results.check(f"power-loss: {power_loss_target} was killed mid-stream",
                  0 < count_at_kill < NUM_READS,
                  f"killed at {count_at_kill}/{NUM_READS}")

    # 2. DBF exact record content, no duplicates after resume.
    dbf_rows = dbf_records(dbf_path)
    dbf_chips = [row["chip"] for row in dbf_rows]
    dbf_events = {row["event"] for row in dbf_rows}
    dbf_readers = {row["reader"] for row in dbf_rows}
    results.expect_eq(f"DBF record count == {NUM_READS}", len(dbf_rows), NUM_READS)
    results.expect_eq("DBF chip IDs match deterministic scenario", dbf_chips, EXPECTED_TAGS)
    results.expect_eq("DBF event type is finish", dbf_events, {"F"})
    results.expect_eq("DBF reader index is preseeded value", dbf_readers, {"0"})
    results.expect_eq("DBF has no duplicate chip rows", len(set(dbf_chips)), len(dbf_chips))

    # 3. Durable TCP local proxy replays exact frames to a fresh client.
    replay = read_proxy_replay(proxy_port, NUM_READS * WIRE_FRAME)
    expected_replay = b"".join(EXPECTED_WIRE)
    results.expect_eq("TCP proxy replays exact deterministic frames",
                      replay, expected_replay)

    # 4. Server announcer state.
    results.check("server announcer generation >= 1",
                  status.get("announcer_source_generation", 0) >= 1,
                  f"generation={status.get('announcer_source_generation')}")
    results.expect_eq(f"server finisher_count == {NUM_READS} (distinct chips)",
                      status.get("finisher_count"), NUM_READS)
    announcer_rows = status.get("announcer_rows", [])
    pushed_chips = {row.get("chip_id") for row in announcer_rows}
    results.check("server announcer rows cover all expected chips",
                  set(EXPECTED_TAGS).issubset(pushed_chips),
                  f"missing={set(EXPECTED_TAGS) - pushed_chips}")
    # Rows carry the DECODED composite stream identity: the forwarder's
    # endpoint id and the wire stream id as separate fields, never the
    # receiver's encoded LocalStreamKey (U+001F separator).
    row_identities = {(row.get("forwarder_endpoint_id"), row.get("stream_id"))
                      for row in announcer_rows}
    results.expect_eq("server announcer rows carry composite stream identity",
                      row_identities, {(forwarder_endpoint_id, stream_id)})
    # Every announcer row must carry a resolved bib and display name (seeded
    # participant + chip data resolved locally on the receiver).
    rows_missing_identity = [
        row.get("chip_id")
        for row in announcer_rows
        if row.get("bib") is None or not row.get("display_name")
    ]
    results.check("server announcer rows carry bib + name",
                  not rows_missing_identity,
                  f"rows missing bib/name: {rows_missing_identity}")

    # DBF stability re-check: a second read must be identical (idempotent rebuild).
    results.expect_eq("DBF record count stable on recheck (no dup rows)",
                      dbf_record_count(dbf_path), NUM_READS)

    if agent_ui_scenario is not None and agent_ui_artifacts_dir is not None:
        emit_agent_ui_artifacts(
            receiver_log_path=active_receiver.log_path,
            scenario_path=agent_ui_scenario,
            artifacts_dir=agent_ui_artifacts_dir,
            expected_stream_id=stream_id,
            expected_local_port=proxy_port,
        )

    return stack


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-build", action="store_true", help="skip cargo build")
    parser.add_argument("--keep", action="store_true",
                        help="keep the temp working dir on exit")
    parser.add_argument(
        "--lcd-sim",
        action="store_true",
        help=(
            "build+run the LCD simulator once against the live forwarder and "
            "assert a PNG is rendered from /api/v1/display-state"
        ),
    )
    parser.add_argument("--power-loss-target", choices=("both", *POWER_LOSS_TARGETS),
                        default="both",
                        help="which SIGKILL+restart lane(s) to run "
                             "(default: both receiver and forwarder)")
    parser.add_argument("--agent-ui-scenario", type=Path,
                        help="optional T5.5 bridge-agent scenario JSON")
    parser.add_argument("--agent-ui-artifacts-dir", type=Path,
                        help="directory for optional T5.5 bridge-agent artifacts")
    args = parser.parse_args()

    if (args.agent_ui_scenario is None) != (args.agent_ui_artifacts_dir is None):
        parser.error("--agent-ui-scenario and --agent-ui-artifacts-dir must be supplied together")

    if not args.no_build:
        cargo_build(agent_ui=args.agent_ui_scenario is not None, lcd_sim=args.lcd_sim)

    targets = resolve_power_loss_targets(args.power_loss_target)
    # The bridge-agent artifacts are emitted once, on the final lane only, so
    # repeated lanes do not clobber each other's artifact directory.
    results = Results()
    all_green = True
    preserved: list[Path] = []

    connections_tmp = Path(tempfile.mkdtemp(prefix="rt-e2e-connections-"))
    print(f"[tmp] working dir (connections scenario): {connections_tmp}")
    connections_results = Results()
    connections_stack = Stack()
    try:
        run_connection_scenarios(
            connections_tmp,
            connections_results,
            connections_stack,
            lcd_sim=args.lcd_sim,
        )
    except Exception as exc:  # noqa: BLE001
        print(f"\n[error] {type(exc).__name__}: {exc}", file=sys.stderr)
        connections_results.check("orchestration completed (connections scenario)", False, str(exc))
    finally:
        try:
            _dump_logs_on_failure(connections_stack, connections_results)
        finally:
            connections_stack.shutdown()

    connections_passed = sum(1 for _, ok, _ in connections_results.checks if ok)
    connections_total = len(connections_results.checks)
    print(
        f"\n=== connections scenario summary: "
        f"{connections_passed}/{connections_total} checks passed ==="
    )
    results.checks.extend(
        (f"[connections] {name}", ok, detail)
        for name, ok, detail in connections_results.checks
    )
    connections_green = connections_results.all_passed and connections_total > 0
    all_green = all_green and connections_green
    if connections_green and not args.keep:
        shutil.rmtree(connections_tmp, ignore_errors=True)
    else:
        preserved.append(connections_tmp)

    for i, target in enumerate(targets):
        last = i == len(targets) - 1
        tmp = Path(tempfile.mkdtemp(prefix=f"rt-e2e-{target}-"))
        print(f"[tmp] working dir ({target} lane): {tmp}")
        lane_results = Results()
        stack = Stack()
        try:
            run(
                tmp,
                lane_results,
                stack,
                power_loss_target=target,
                agent_ui_scenario=args.agent_ui_scenario if last else None,
                agent_ui_artifacts_dir=args.agent_ui_artifacts_dir if last else None,
            )
        except Exception as exc:  # noqa: BLE001
            print(f"\n[error] {type(exc).__name__}: {exc}", file=sys.stderr)
            lane_results.check(f"orchestration completed ({target} lane)", False, str(exc))
        finally:
            try:
                _dump_logs_on_failure(stack, lane_results)
            finally:
                stack.shutdown()

        lane_passed = sum(1 for _, ok, _ in lane_results.checks if ok)
        lane_total = len(lane_results.checks)
        print(f"\n=== {target} lane summary: {lane_passed}/{lane_total} checks passed ===")
        results.checks.extend(
            (f"[{target}] {name}", ok, detail) for name, ok, detail in lane_results.checks
        )
        lane_green = lane_results.all_passed and lane_total > 0
        all_green = all_green and lane_green
        if lane_green and not args.keep:
            shutil.rmtree(tmp, ignore_errors=True)
        else:
            preserved.append(tmp)

    print("\n=== Summary (all lanes) ===")
    passed = sum(1 for _, ok, _ in results.checks if ok)
    total = len(results.checks)
    print(f"  scenarios: connections, {', '.join(targets)}")
    print(f"  {passed}/{total} checks passed")

    if all_green and total > 0:
        print("\nE2E STACK: GREEN")
        return 0

    print("\nE2E STACK: RED")
    for tmp in preserved:
        print(f"  logs and config preserved in {tmp}")
    return 1


def _dump_logs_on_failure(stack: Stack, results: Results):
    if results.all_passed:
        return
    print("\n=== Process log tails (failure diagnostics) ===")
    for managed in stack.procs:
        if not managed.log_path.exists():
            continue
        print(f"\n----- {managed.name} ({managed.log_path}) -----")
        lines = managed.log_path.read_text(errors="replace").splitlines()
        for line in lines[-40:]:
            print(f"  {line}")


if __name__ == "__main__":
    sys.exit(main())
