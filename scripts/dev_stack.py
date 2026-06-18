#!/usr/bin/env python3
"""Manual local P2P dev stack — boot everything and interact by hand.

This is the development convenience runner (the P2P-era successor to the old
pre-cutover dev helper). Unlike ``scripts/e2e/run_stack.py`` it runs **no
assertions and no power-loss lanes**: it just brings the whole loopback stack up
and leaves it running so you can click around in the receiver UI and watch reads
flow.

It starts, on loopback only:

    emulator  --(TCP)-->  forwarder  --(iroh P2P)-->  receiver (desktop app)
                                                           |
                              thin-node  <--(announcer push)+

The iroh transport is configured deterministically (relays disabled, discovery
off, seeded keys, injected direct addresses, static allow-list) so the receiver
connects to the forwarder with no external network. The receiver's SQLite DB is
preseeded with one subscription to the emulated stream, so a stream and live
reads appear immediately; you can still add/remove subscriptions in the UI.

Receiver modes (``--receiver``):

* ``tauri``    (default) launch the real desktop app via ``cargo tauri dev``.
* ``headless`` launch ``receiver-headless`` (no GUI; control API + data plane).
* ``none``     set everything up and print the exact launch command + env, but
               do not spawn a receiver (launch it yourself).

The desktop app reads its P2P config from the ``RT_P2P_*`` / ``RT_RECEIVER_*``
environment variables this script sets (see ``services/receiver`` and
``apps/receiver-ui/src-tauri``). Press Ctrl-C to tear the whole stack down.

Usage::

    uv run scripts/dev_stack.py                 # build + launch desktop app
    uv run scripts/dev_stack.py --receiver headless
    uv run scripts/dev_stack.py --receiver none
    uv run scripts/dev_stack.py --no-build      # skip cargo build
    uv run scripts/dev_stack.py --read-delay-ms 500
    uv run scripts/dev_stack.py --data-dir /tmp/rt-dev   # reuse a data dir

Stdlib only; uses ``cargo`` to build the service binaries and ``cargo tauri``
to launch the desktop app.
"""

from __future__ import annotations

import argparse
import contextlib
import os
import shutil
import signal
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
TARGET_DIR = REPO_ROOT / "target" / "debug"
RECEIVER_UI_DIR = REPO_ROOT / "apps" / "receiver-ui"

# Deterministic 32-byte secret-key seeds (hex). Distinct so the two endpoints
# get distinct node ids. Loopback-only; never used outside development.
FORWARDER_SEED_HEX = "cd" * 32
RECEIVER_SEED_HEX = "ab" * 32

PROVISIONING_TOKEN = "dev-provisioning-token"

# Reads file shape (looped forever by the emulator).
NUM_FRAMES = 25


# ---------------------------------------------------------------------------
# Deterministic IPICO frame construction (mirrors ipico_core::read checksum).
# ---------------------------------------------------------------------------
def build_frame(chip_index: int) -> str:
    tag = format(chip_index, "012x")
    core = "aa" + "40" + tag + "0a2a" + "01" + "12" + "30" + "18" + "45" + "59" + "00"
    checksum = sum(ord(c) for c in core[2:34]) % 256
    return core + format(checksum, "02x")


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


def bin_path(name: str) -> Path:
    path = TARGET_DIR / name
    if not path.exists():
        raise FileNotFoundError(
            f"missing built binary {path}; run without --no-build (or `cargo build`)"
        )
    return path


def derive_node_id(seed_hex: str) -> str:
    out = subprocess.run(
        [
            str(bin_path("receiver-headless")),
            "print-node-id",
            "--p2p-secret-key-seed-hex",
            seed_hex,
        ],
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
# Process management
# ---------------------------------------------------------------------------
@dataclass
class Managed:
    name: str
    argv: list[str]
    log_path: Path
    env: dict | None = None
    cwd: Path | None = None
    proc: subprocess.Popen | None = None
    log_fh: object | None = None

    def start(self) -> None:
        self.log_fh = open(self.log_path, "w")  # noqa: SIM115 - closed in stop()
        env = dict(os.environ)
        if self.env:
            env.update(self.env)
        print(f"[start] {self.name} (log: {self.log_path})")
        self.proc = subprocess.Popen(
            self.argv,
            stdout=self.log_fh,
            stderr=subprocess.STDOUT,
            cwd=str(self.cwd or REPO_ROOT),
            env=env,
        )

    def stop(self) -> None:
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                with contextlib.suppress(Exception):
                    self.proc.wait(timeout=8)
        if self.log_fh:
            with contextlib.suppress(Exception):
                self.log_fh.close()
            self.log_fh = None

    def assert_alive(self) -> None:
        if self.proc and self.proc.poll() is not None:
            raise RuntimeError(
                f"{self.name} exited early with code {self.proc.returncode}; "
                f"see {self.log_path}"
            )


class Stack:
    def __init__(self) -> None:
        self.procs: list[Managed] = []

    def add(self, managed: Managed) -> Managed:
        self.procs.append(managed)
        return managed

    def shutdown(self) -> None:
        for managed in reversed(self.procs):
            managed.stop()


def wait_for_log(path: Path, needle: str, timeout: float, what: str) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists() and needle in path.read_text(errors="replace"):
            return
        time.sleep(0.1)
    raise TimeoutError(f"timed out waiting for {what} (looking for {needle!r} in {path})")


def tcp_ready(port: int) -> bool:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.settimeout(0.25)
        return s.connect_ex(("127.0.0.1", port)) == 0


def wait_tcp(port: int, timeout: float, what: str) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if tcp_ready(port):
            return
        time.sleep(0.1)
    raise TimeoutError(f"timed out waiting for {what} on 127.0.0.1:{port}")


# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
def cargo_build() -> None:
    # Build the forwarder UI first so the forwarder can embed it (the forwarder
    # serves its web UI only when built with `--features embed-ui`, which embeds
    # `apps/forwarder-ui/build` at compile time).
    print("[build] forwarder UI (npm run build --workspace apps/forwarder-ui) ...")
    subprocess.run(
        ["npm", "run", "build", "--workspace", "apps/forwarder-ui"],
        cwd=str(REPO_ROOT),
        check=True,
    )
    print("[build] cargo build -p emulator -p thin-node -p receiver ...")
    subprocess.run(
        ["cargo", "build", "-p", "emulator", "-p", "thin-node", "-p", "receiver"],
        cwd=str(REPO_ROOT),
        check=True,
    )
    print("[build] cargo build -p forwarder --features embed-ui ...")
    subprocess.run(
        ["cargo", "build", "-p", "forwarder", "--features", "embed-ui"],
        cwd=str(REPO_ROOT),
        check=True,
    )


# ---------------------------------------------------------------------------
# Receiver DB preseed (canonical stream subscription + DBF profile)
# ---------------------------------------------------------------------------
PRESEED_SQL = """
CREATE TABLE IF NOT EXISTS profile (
    thin_node_url TEXT NOT NULL,
    token         TEXT NOT NULL,
    update_mode   TEXT NOT NULL DEFAULT 'check-and-download',
    receiver_mode_json TEXT,
    receiver_id   TEXT,
    dbf_enabled   INTEGER NOT NULL DEFAULT 0,
    dbf_path      TEXT NOT NULL DEFAULT 'C:\\winrace\\Files\\IPICO.DBF'
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


def preseed_receiver_db(
    db_path: Path,
    *,
    thin_node_url: str,
    forwarder_node_id: str,
    stream_id: str,
    proxy_port: int,
    dbf_path: Path,
    receiver_id: str,
) -> None:
    db_path.parent.mkdir(parents=True, exist_ok=True)
    conn = sqlite3.connect(str(db_path))
    try:
        conn.executescript(PRESEED_SQL)
        # Re-seed idempotently so re-running with the same --data-dir is clean.
        conn.execute("DELETE FROM profile")
        conn.execute(
            "INSERT INTO profile "
            "(thin_node_url, token, update_mode, receiver_mode_json, receiver_id, "
            " dbf_enabled, dbf_path) VALUES (?,?,?,?,?,?,?)",
            (thin_node_url, PROVISIONING_TOKEN, "check-and-download", None,
             receiver_id, 1, str(dbf_path)),
        )
        conn.execute(
            "INSERT OR REPLACE INTO subscriptions "
            "(forwarder_endpoint_id, stream_id, local_port_override, event_type, "
            " forwarder_id, reader_ip) VALUES (?,?,?,?,?,?)",
            (forwarder_node_id, stream_id, proxy_port, "finish", None, stream_id),
        )
        conn.commit()
    finally:
        conn.close()


def write_forwarder_config(
    path: Path,
    *,
    token_file: Path,
    journal_path: Path,
    status_port: int,
    emulator_port: int,
    fanout_port: int,
    p2p_port: int,
    receiver_node_id: str,
) -> None:
    path.write_text(
        f"""schema_version = 1
display_name = "Dev Forwarder"

[auth]
token_file = "{token_file}"

[journal]
sqlite_path = "{journal_path}"

[status_http]
bind = "127.0.0.1:{status_port}"

[[readers]]
target = "127.0.0.1:{emulator_port}"
enabled = true
local_fallback_port = {fanout_port}

[p2p]
enabled = true
secret_key_seed_hex = "{FORWARDER_SEED_HEX}"
bind_addr_v4 = "127.0.0.1:{p2p_port}"
relay_disabled = true
discovery_disabled = true
max_concurrent_bidi_streams = 256
static_allowed_receivers = ["{receiver_node_id}"]
"""
    )


# ---------------------------------------------------------------------------
# Orchestration
# ---------------------------------------------------------------------------
def main() -> int:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--receiver",
        choices=("tauri", "headless", "none"),
        default="tauri",
        help="how to run the receiver (default: tauri desktop app)",
    )
    parser.add_argument("--no-build", action="store_true", help="skip cargo build")
    parser.add_argument(
        "--data-dir",
        type=Path,
        help="receiver data dir (default: a fresh temp dir, kept on exit)",
    )
    parser.add_argument(
        "--read-delay-ms",
        type=int,
        default=1000,
        help="delay between emulated reads in ms (default: 1000)",
    )
    parser.add_argument(
        "--keep",
        action="store_true",
        help="keep the temp working dir on exit (implied when --data-dir is set)",
    )
    args = parser.parse_args()

    if not args.no_build:
        cargo_build()

    work_dir = Path(tempfile.mkdtemp(prefix="rt-dev-stack-"))
    keep = args.keep or args.data_dir is not None
    receiver_data_dir = (args.data_dir or (work_dir / "receiver-data")).resolve()
    receiver_data_dir.mkdir(parents=True, exist_ok=True)

    # --- Ports (loopback only) ---
    emulator_port = free_tcp_port()
    forwarder_status_port = free_tcp_port()
    forwarder_fanout_port = free_tcp_port()
    forwarder_p2p_port = free_udp_port()
    thin_node_port = free_tcp_port()
    proxy_port = free_tcp_port()

    stream_id = f"127.0.0.1:{emulator_port}"
    thin_node_url = f"http://127.0.0.1:{thin_node_port}"
    receiver_id = "dev-receiver"

    forwarder_node_id = derive_node_id(FORWARDER_SEED_HEX)
    receiver_node_id = derive_node_id(RECEIVER_SEED_HEX)

    # --- Files ---
    reads_file = work_dir / "reads.txt"
    reads_file.write_text(
        "\n".join(build_frame(i) for i in range(1, NUM_FRAMES + 1)) + "\n"
    )
    token_file = work_dir / "forwarder-token"
    token_file.write_text("dev-forwarder-token\n")
    journal_path = work_dir / "forwarder.sqlite3"
    thin_db_path = work_dir / "thin-node.sqlite3"
    dbf_path = work_dir / "IPICO.DBF"
    receiver_db_path = receiver_data_dir / "receiver.sqlite3"
    forwarder_config = work_dir / "forwarder.toml"

    write_forwarder_config(
        forwarder_config,
        token_file=token_file,
        journal_path=journal_path,
        status_port=forwarder_status_port,
        emulator_port=emulator_port,
        fanout_port=forwarder_fanout_port,
        p2p_port=forwarder_p2p_port,
        receiver_node_id=receiver_node_id,
    )
    preseed_receiver_db(
        receiver_db_path,
        thin_node_url=thin_node_url,
        forwarder_node_id=forwarder_node_id,
        stream_id=stream_id,
        proxy_port=proxy_port,
        dbf_path=dbf_path,
        receiver_id=receiver_id,
    )

    # The RT_* env the desktop app / receiver-headless read to start P2P.
    receiver_env = {
        "RT_RECEIVER_DATA_DIR": str(receiver_data_dir),
        "RT_RECEIVER_ID": receiver_id,
        "RT_P2P_FORWARDER_NODE_ID": forwarder_node_id,
        "RT_P2P_FORWARDER_DIRECT_ADDR": f"127.0.0.1:{forwarder_p2p_port}",
        "RT_P2P_SECRET_KEY_SEED_HEX": RECEIVER_SEED_HEX,
        "RT_P2P_THIN_NODE_URL": thin_node_url,
        "RT_P2P_THIN_NODE_TOKEN": PROVISIONING_TOKEN,
        "RT_P2P_RECONCILE_MS": "1000",
        "RUST_LOG": os.environ.get("RUST_LOG", "info,receiver=debug"),
    }

    stack = Stack()
    try:
        # --- thin-node ---
        thin = stack.add(Managed(
            name="thin-node",
            argv=[str(bin_path("thin-node"))],
            log_path=work_dir / "thin-node.log",
            env={
                "THIN_NODE_DB_PATH": str(thin_db_path),
                "BIND_ADDR": f"127.0.0.1:{thin_node_port}",
                "THIN_NODE_PROVISIONING_TOKEN": PROVISIONING_TOKEN,
                "LOG_LEVEL": "info",
            },
        ))
        thin.start()
        wait_tcp(thin_node_port, timeout=20, what="thin-node")
        thin.assert_alive()

        # --- emulator (loops the reads file forever) ---
        emulator = stack.add(Managed(
            name="emulator",
            argv=[
                str(bin_path("emulator")),
                "-p", str(emulator_port),
                "-f", str(reads_file),
                "-d", str(args.read_delay_ms),
                "-t", "raw",
            ],
            log_path=work_dir / "emulator.log",
            env={"RUST_LOG": "info"},
        ))
        emulator.start()
        wait_for_log(emulator.log_path, "listening on", timeout=20,
                     what="emulator TCP listener")
        emulator.assert_alive()

        # --- forwarder (P2P, seeded, relay/discovery off, static allow-list) ---
        forwarder = stack.add(Managed(
            name="forwarder",
            argv=[str(bin_path("forwarder")), "--config", str(forwarder_config)],
            log_path=work_dir / "forwarder.log",
            env={"RUST_LOG": "info,forwarder=debug"},
        ))
        forwarder.start()
        wait_for_log(forwarder.log_path, "p2p iroh server started", timeout=30,
                     what="forwarder p2p startup")
        forwarder.assert_alive()

        print_summary(
            work_dir=work_dir,
            receiver_data_dir=receiver_data_dir,
            thin_node_url=thin_node_url,
            forwarder_status_port=forwarder_status_port,
            emulator_port=emulator_port,
            proxy_port=proxy_port,
            stream_id=stream_id,
            dbf_path=dbf_path,
            forwarder_node_id=forwarder_node_id,
            receiver_node_id=receiver_node_id,
            receiver_env=receiver_env,
            mode=args.receiver,
        )

        run_receiver(args.receiver, stack, work_dir, receiver_env)
    except KeyboardInterrupt:
        print("\n[shutdown] Ctrl-C received; tearing down the stack ...")
    finally:
        stack.shutdown()
        if keep:
            print(f"[cleanup] preserved working dir: {work_dir}")
            print(f"[cleanup] preserved receiver data dir: {receiver_data_dir}")
        else:
            shutil.rmtree(work_dir, ignore_errors=True)

    return 0


def run_receiver(mode: str, stack: Stack, work_dir: Path, receiver_env: dict) -> None:
    if mode == "none":
        print("\n[receiver] mode=none: stack is up. Launch a receiver yourself with "
              "the env above, then Ctrl-C here to tear everything down.")
        signal.pause()
        return

    if mode == "headless":
        receiver = Managed(
            name="receiver-headless",
            argv=[
                str(bin_path("receiver-headless")),
                "--data-dir", receiver_env["RT_RECEIVER_DATA_DIR"],
                "--bind-addr", "127.0.0.1:0",
                "--receiver-id", receiver_env["RT_RECEIVER_ID"],
                "--p2p-forwarder-node-id", receiver_env["RT_P2P_FORWARDER_NODE_ID"],
                "--p2p-forwarder-direct-addr", receiver_env["RT_P2P_FORWARDER_DIRECT_ADDR"],
                "--p2p-secret-key-seed-hex", receiver_env["RT_P2P_SECRET_KEY_SEED_HEX"],
                "--p2p-thin-node-url", receiver_env["RT_P2P_THIN_NODE_URL"],
                "--p2p-thin-node-token", receiver_env["RT_P2P_THIN_NODE_TOKEN"],
                "--p2p-reconcile-ms", receiver_env["RT_P2P_RECONCILE_MS"],
            ],
            log_path=work_dir / "receiver-headless.log",
            env={"RUST_LOG": receiver_env["RUST_LOG"]},
        )
        print("\n[receiver] launching receiver-headless in the foreground "
              "(Ctrl-C to stop the whole stack) ...")
        # Foreground: inherit stdout/stderr so the user sees its log live.
        env = dict(os.environ)
        env.update(receiver_env)
        proc = subprocess.Popen(receiver.argv, cwd=str(REPO_ROOT), env=env)
        stack.procs.append(Managed(name="receiver-headless", argv=receiver.argv,
                                   log_path=receiver.log_path, proc=proc))
        proc.wait()
        return

    # mode == "tauri": launch the real desktop app via `cargo tauri dev`.
    print("\n[receiver] launching the desktop app via `cargo tauri dev` "
          "(Ctrl-C to stop the whole stack) ...")
    env = dict(os.environ)
    env.update(receiver_env)
    proc = subprocess.Popen(
        ["cargo", "tauri", "dev"],
        cwd=str(RECEIVER_UI_DIR),
        env=env,
    )
    stack.procs.append(Managed(name="receiver-tauri",
                               argv=["cargo", "tauri", "dev"],
                               log_path=work_dir / "receiver-tauri.log", proc=proc))
    proc.wait()


def print_summary(**kw) -> None:
    env = kw["receiver_env"]
    print("\n" + "=" * 70)
    print("  Rusty Timer — local P2P dev stack is UP")
    print("=" * 70)
    print(f"  Thin-node status:   {kw['thin_node_url']}/status   (JSON; no web UI yet)")
    print(f"  Thin-node health:   {kw['thin_node_url']}/healthz")
    print(f"  Forwarder UI:       http://127.0.0.1:{kw['forwarder_status_port']}/   (status API at /api/v1/status)")
    print(f"  Emulator (reads):   127.0.0.1:{kw['emulator_port']}   (log: {kw['work_dir']}/emulator.log)")
    print(f"  Local TCP output:   127.0.0.1:{kw['proxy_port']}  (timing-software feed)")
    print(f"  Stream id:          {kw['stream_id']}")
    print(f"  Forwarder node id:  {kw['forwarder_node_id'][:16]}...")
    print(f"  Receiver node id:   {kw['receiver_node_id'][:16]}...")
    print(f"  Receiver data dir:  {kw['receiver_data_dir']}")
    print(f"  DBF output:         {kw['dbf_path']}")
    print(f"  Logs / work dir:    {kw['work_dir']}")
    print("-" * 70)
    print("  Receiver env (read by the desktop app / receiver-headless):")
    for key in (
        "RT_RECEIVER_DATA_DIR", "RT_RECEIVER_ID", "RT_P2P_FORWARDER_NODE_ID",
        "RT_P2P_FORWARDER_DIRECT_ADDR", "RT_P2P_SECRET_KEY_SEED_HEX",
        "RT_P2P_THIN_NODE_URL", "RT_P2P_THIN_NODE_TOKEN", "RT_P2P_RECONCILE_MS",
    ):
        print(f"    {key}={env[key]}")
    if kw["mode"] == "none":
        cmd = " ".join(f'{k}="{env[k]}"' for k in (
            "RT_RECEIVER_DATA_DIR", "RT_RECEIVER_ID", "RT_P2P_FORWARDER_NODE_ID",
            "RT_P2P_FORWARDER_DIRECT_ADDR", "RT_P2P_SECRET_KEY_SEED_HEX",
            "RT_P2P_THIN_NODE_URL", "RT_P2P_THIN_NODE_TOKEN", "RT_P2P_RECONCILE_MS",
        ))
        print("-" * 70)
        print("  Launch the desktop app yourself with:")
        print(f"    cd {RECEIVER_UI_DIR}")
        print(f"    {cmd} cargo tauri dev")
    print("=" * 70 + "\n")


if __name__ == "__main__":
    sys.exit(main())
