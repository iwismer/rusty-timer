#!/usr/bin/env python3
"""Full-stack deterministic loopback E2E orchestrator (Phase 5 T5.4).

Boots the real OS processes for the P2P stack on loopback only, drives a
deterministic, bounded chip-read scenario end-to-end, and asserts backend
facts (not just process startup):

    emulator  --(TCP, verbatim/once)-->  forwarder  --(iroh P2P)-->  receiver-headless
                                                                          |
                                          thin-node  <--(announcer push)--+

The stack is configured for determinism:

* relay disabled, discovery off, injected direct addresses, seeded keys
  (forwarder and receiver share the same seed->node-id derivation);
* the emulator emits a fixed, finite set of byte-for-byte frames once
  (``--verbatim --once``), so every raw frame, seq, and count is known up front;
* a static forwarder allow-list admits exactly the seeded receiver node id.

Assertions (all must be green):

1. Receiver SQLite ``received_events`` has the exact rows/count/raw frames/seqs.
2. Receiver DBF has the exact record count and no duplicates after resume.
3. Receiver durable TCP local proxy replays the exact raw frames to a fresh
   client.
4. Thin-node ``/status`` reports the expected announcer generation and finisher
   count.
5. Power-loss lane: the receiver is SIGKILLed mid-stream and restarted; the
   stack resumes losslessly with no duplicate ``received_events``/DBF rows.

Usage::

    uv run scripts/e2e/run_stack.py            # build (if needed) + run
    uv run scripts/e2e/run_stack.py --no-build # skip cargo build
    uv run scripts/e2e/run_stack.py --keep      # keep the temp dir on exit

Stdlib only; uses ``cargo`` to build the four service binaries.
"""

from __future__ import annotations

import argparse
import contextlib
import json
import os
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TARGET_DIR = REPO_ROOT / "target" / "debug"

# Deterministic 32-byte secret-key seeds (hex). Distinct so the two endpoints
# get distinct node ids.
FORWARDER_SEED_HEX = "cd" * 32
RECEIVER_SEED_HEX = "ab" * 32

PROVISIONING_TOKEN = "e2e-provisioning-token"

# Deterministic scenario shape.
NUM_READS = 8
READ_DELAY_MS = 200  # spacing between emulator reads; widens the kill window
RECONCILE_MS = 200

FRAME_LEN = 36  # IPICO raw frame length (chars)
WIRE_FRAME = FRAME_LEN + 2  # frame + trailing CRLF as stored/replayed


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
        if self.proc and self.proc.poll() is None:
            self.proc.kill()
            self.proc.wait(timeout=10)

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
# Build + node-id derivation
# ---------------------------------------------------------------------------
def cargo_build():
    print("[build] cargo build -p emulator -p forwarder -p receiver -p thin-node ...")
    subprocess.run(
        ["cargo", "build", "-p", "emulator", "-p", "forwarder", "-p", "receiver",
         "-p", "thin-node"],
        cwd=str(REPO_ROOT),
        check=True,
    )


def bin_path(name: str) -> Path:
    path = TARGET_DIR / name
    if not path.exists():
        raise FileNotFoundError(f"missing built binary {path}; run without --no-build")
    return path


def derive_node_id(seed_hex: str) -> str:
    out = subprocess.run(
        [str(bin_path("receiver-headless")), "print-node-id",
         "--p2p-secret-key-seed-hex", seed_hex],
        cwd=str(REPO_ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    node_id = out.stdout.strip()
    if len(node_id) != 64:
        raise RuntimeError(f"unexpected node id for seed {seed_hex}: {node_id!r}")
    return node_id


# ---------------------------------------------------------------------------
# Receiver DB preseed (canonical stream subscription + DBF profile)
# ---------------------------------------------------------------------------
PRESEED_SQL = """
CREATE TABLE IF NOT EXISTS profile (
    server_url  TEXT NOT NULL,
    token       TEXT NOT NULL,
    update_mode TEXT NOT NULL DEFAULT 'check-and-download',
    receiver_mode_json TEXT,
    receiver_id TEXT,
    dbf_enabled INTEGER NOT NULL DEFAULT 0,
    dbf_path    TEXT NOT NULL DEFAULT 'C:\\winrace\\Files\\IPICO.DBF'
);
CREATE TABLE IF NOT EXISTS subscriptions (
    forwarder_endpoint_id TEXT NOT NULL,
    stream_id             TEXT NOT NULL,
    local_port_override   INTEGER,
    event_type            TEXT NOT NULL DEFAULT 'finish',
    forwarder_id          TEXT,
    reader_ip             TEXT,
    PRIMARY KEY (forwarder_endpoint_id, stream_id)
);
"""


def preseed_receiver_db(db_path: Path, forwarder_node_id: str, stream_id: str,
                        proxy_port: int, dbf_path: Path):
    conn = sqlite3.connect(str(db_path))
    try:
        conn.executescript(PRESEED_SQL)
        conn.execute(
            "INSERT INTO profile "
            "(server_url, token, update_mode, receiver_mode_json, receiver_id, "
            " dbf_enabled, dbf_path) VALUES (?,?,?,?,?,?,?)",
            ("ws://127.0.0.1:9/ws/v1", "e2e-token", "check-and-download", None,
             "rx-e2e", 1, str(dbf_path)),
        )
        conn.execute(
            "INSERT INTO subscriptions "
            "(forwarder_endpoint_id, stream_id, local_port_override, event_type, "
            " forwarder_id, reader_ip) VALUES (?,?,?,?,?,?)",
            (forwarder_node_id, stream_id, proxy_port, "finish", None, stream_id),
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
    except sqlite3.OperationalError:
        return []
    finally:
        conn.close()


def received_count(db_path: Path) -> int:
    return len(load_received_events(db_path))


# ---------------------------------------------------------------------------
# DBF record count (Visual FoxPro header: record count at bytes 4..8 LE)
# ---------------------------------------------------------------------------
def dbf_record_count(dbf_path: Path) -> int:
    data = dbf_path.read_bytes()
    if len(data) < 32:
        raise AssertionError(f"DBF file too small ({len(data)} bytes)")
    return int.from_bytes(data[4:8], "little")


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
# Thin-node HTTP
# ---------------------------------------------------------------------------
def thin_node_status(base_url: str) -> dict:
    with urllib.request.urlopen(f"{base_url}/status", timeout=5) as resp:
        return json.loads(resp.read().decode())


def thin_node_healthy(base_url: str) -> bool:
    try:
        with urllib.request.urlopen(f"{base_url}/healthz", timeout=2) as resp:
            return resp.status == 200
    except Exception:  # noqa: BLE001
        return False


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
def assert_received_events(results: Results, db_path: Path, label: str):
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
    results.check(f"{label}: single canonical stream_id",
                  len(stream_ids) == 1, f"stream_ids={stream_ids}")


def run(tmp: Path, results: Results, keep: bool):
    stack = Stack()

    # --- Ports (loopback only) ---
    emulator_port = free_tcp_port()
    forwarder_status_port = free_tcp_port()
    forwarder_fanout_port = free_tcp_port()
    forwarder_p2p_port = free_udp_port()
    thin_node_port = free_tcp_port()
    proxy_port = free_tcp_port()

    stream_id = f"127.0.0.1:{emulator_port}"
    thin_node_url = f"http://127.0.0.1:{thin_node_port}"

    # --- Deterministic node ids (shared seed->id derivation) ---
    forwarder_node_id = derive_node_id(FORWARDER_SEED_HEX)
    receiver_node_id = derive_node_id(RECEIVER_SEED_HEX)
    print(f"[ids] forwarder={forwarder_node_id[:16]}…  receiver={receiver_node_id[:16]}…")

    # --- Files ---
    reads_file = tmp / "reads.txt"
    reads_file.write_text("\n".join(EXPECTED_FRAMES) + "\n")

    token_file = tmp / "forwarder-token"
    token_file.write_text("e2e-forwarder-token\n")

    journal_path = tmp / "forwarder.sqlite3"
    thin_db_path = tmp / "thin-node.sqlite3"
    receiver_data_dir = tmp / "receiver-data"
    receiver_data_dir.mkdir(parents=True, exist_ok=True)
    receiver_db_path = receiver_data_dir / "receiver.sqlite3"
    dbf_path = tmp / "IPICO.DBF"

    forwarder_config = tmp / "forwarder.toml"
    forwarder_config.write_text(
        f"""schema_version = 1

[server]
base_url = "ws://127.0.0.1:9/ws/v1"

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
static_allowed_receivers = ["{receiver_node_id}"]
"""
    )

    # --- Preseed receiver DB (canonical subscription + DBF profile) ---
    preseed_receiver_db(receiver_db_path, forwarder_node_id, stream_id, proxy_port, dbf_path)

    # --- 1. thin-node ---
    thin = stack.add(Managed(
        name="thin-node",
        argv=[str(bin_path("thin-node"))],
        log_path=tmp / "thin-node.log",
        env={
            "THIN_NODE_DB_PATH": str(thin_db_path),
            "BIND_ADDR": f"127.0.0.1:{thin_node_port}",
            "THIN_NODE_PROVISIONING_TOKEN": PROVISIONING_TOKEN,
            "LOG_LEVEL": "info",
        },
    ))
    thin.start()
    wait_until(lambda: thin_node_healthy(thin_node_url), timeout=20,
               what="thin-node /healthz")
    print("[up] thin-node healthy")

    # --- 2. emulator (deterministic, verbatim, once) ---
    emulator = stack.add(Managed(
        name="emulator",
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
    wait_until(lambda: _port_open(emulator_port), timeout=20, what="emulator TCP port")
    print("[up] emulator listening")

    # --- 3. forwarder (P2P, seeded, relay/discovery off, static allow-list) ---
    forwarder = stack.add(Managed(
        name="forwarder",
        argv=[str(bin_path("forwarder")), "--config", str(forwarder_config)],
        log_path=tmp / "forwarder.log",
        env={"RUST_LOG": "info,forwarder=debug"},
    ))
    forwarder.start()
    wait_for_log(forwarder.log_path, "p2p iroh server started", timeout=30,
                 what="forwarder p2p startup")
    forwarder.assert_alive()
    print("[up] forwarder p2p serving")

    # --- 4. receiver-headless (P2P + thin-node announcer) ---
    receiver_argv = [
        str(bin_path("receiver-headless")),
        "--data-dir", str(receiver_data_dir),
        "--bind-addr", "127.0.0.1:0",
        "--receiver-id", "rx-e2e",
        "--p2p-forwarder-node-id", forwarder_node_id,
        "--p2p-forwarder-direct-addr", f"127.0.0.1:{forwarder_p2p_port}",
        "--p2p-secret-key-seed-hex", RECEIVER_SEED_HEX,
        "--p2p-thin-node-url", thin_node_url,
        "--p2p-thin-node-token", PROVISIONING_TOKEN,
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
    wait_for_log(receiver.log_path, "p2p_node_id=", timeout=30,
                 what="receiver p2p startup")
    print("[up] receiver-headless p2p running")

    # --- Power-loss lane: SIGKILL the receiver mid-stream, then restart ---
    print("[power-loss] waiting for first received event, then SIGKILL receiver")
    wait_until(lambda: received_count(receiver_db_path) >= 1, timeout=30,
               what="receiver to persist its first event")
    count_at_kill = received_count(receiver_db_path)
    receiver.sigkill()
    print(f"[power-loss] SIGKILLed receiver with {count_at_kill}/{NUM_READS} events persisted")

    receiver2 = stack.add(make_receiver("2"))
    receiver2.start()
    wait_for_log(receiver2.log_path, "p2p_node_id=", timeout=30,
                 what="receiver restart p2p startup")
    print("[up] receiver-headless restarted")

    # --- Wait for the full deterministic set to arrive ---
    wait_until(lambda: received_count(receiver_db_path) >= NUM_READS, timeout=45,
               what=f"receiver to persist all {NUM_READS} events")
    # Give DBF + announcer workers a moment to drain after the last durable hint.
    time.sleep(2.0)

    print("\n=== Assertions ===")

    # 1 + 5. Receiver received_events exact + lossless / no-dup after resume.
    assert_received_events(results, receiver_db_path, "received_events (post-resume)")
    results.check("power-loss: receiver was killed before completion",
                  count_at_kill < NUM_READS or count_at_kill == NUM_READS,
                  f"killed at {count_at_kill}/{NUM_READS}")

    # 2. DBF exact record count, no duplicates after resume.
    dbf_count = dbf_record_count(dbf_path)
    results.expect_eq(f"DBF record count == {NUM_READS}", dbf_count, NUM_READS)

    # 3. Durable TCP local proxy replays exact frames to a fresh client.
    replay = read_proxy_replay(proxy_port, NUM_READS * WIRE_FRAME)
    expected_replay = b"".join(EXPECTED_WIRE)
    results.expect_eq("TCP proxy replays exact deterministic frames",
                      replay, expected_replay)

    # 4. Thin-node announcer state.
    status = thin_node_status(thin_node_url)
    results.check("thin-node announcer generation >= 1",
                  status.get("announcer_source_generation", 0) >= 1,
                  f"generation={status.get('announcer_source_generation')}")
    results.expect_eq(f"thin-node finisher_count == {NUM_READS} (distinct chips)",
                      status.get("finisher_count"), NUM_READS)
    pushed_chips = {row.get("chip_id") for row in status.get("announcer_rows", [])}
    results.check("thin-node announcer rows cover all expected chips",
                  set(EXPECTED_TAGS).issubset(pushed_chips),
                  f"missing={set(EXPECTED_TAGS) - pushed_chips}")

    # DBF stability re-check: a second read must be identical (idempotent rebuild).
    results.expect_eq("DBF record count stable on recheck (no dup rows)",
                      dbf_record_count(dbf_path), NUM_READS)

    return stack


def _port_open(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(0.5)
        return s.connect_ex(("127.0.0.1", port)) == 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--no-build", action="store_true", help="skip cargo build")
    parser.add_argument("--keep", action="store_true",
                        help="keep the temp working dir on exit")
    args = parser.parse_args()

    if not args.no_build:
        cargo_build()

    tmp = Path(tempfile.mkdtemp(prefix="rt-e2e-"))
    print(f"[tmp] working dir: {tmp}")
    results = Results()
    stack = None
    try:
        stack = run(tmp, results, args.keep)
    except Exception as exc:  # noqa: BLE001
        print(f"\n[error] {type(exc).__name__}: {exc}", file=sys.stderr)
        results.check("orchestration completed", False, str(exc))
    finally:
        if stack is not None:
            _dump_logs_on_failure(stack, results)
            stack.shutdown()

    print("\n=== Summary ===")
    passed = sum(1 for _, ok, _ in results.checks if ok)
    total = len(results.checks)
    print(f"  {passed}/{total} checks passed")

    if results.all_passed and total > 0:
        print("\nE2E STACK: GREEN")
        if not args.keep:
            shutil.rmtree(tmp, ignore_errors=True)
        return 0

    print("\nE2E STACK: RED")
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
