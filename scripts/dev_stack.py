#!/usr/bin/env python3
"""Manual local P2P dev stack — boot everything and interact by hand.

This is the development convenience runner (the P2P-era successor to the old
pre-cutover dev helper). Unlike ``scripts/e2e/run_stack.py`` it runs **no
assertions and no power-loss lanes**: it brings the whole loopback stack up,
the prod-like way, and leaves it running so you can drive it through the web
UIs.

It starts, on loopback only:

    emulator  --(TCP)-->  forwarder  --(iroh P2P)-->  receiver (desktop app)
                                                           |
                              server  <--(register / catalog / announcer)--+

The flow mirrors production (no static allow-list, no preseeded subscription,
no hand-fed node ids):

1. The forwarder and receiver each **self-register** with the server (TOFU)
   using a deterministic seeded identity, and the forwarder pushes its stream
   catalog + direct addresses.
2. You open the **server UI** and approve BOTH the forwarder and the receiver
   (TOFU approve + name).
3. The forwarder fetches the receiver allow-list; the receiver discovers the
   approved forwarder (node id + direct addresses) from the server.
4. You open the **receiver UI**, see the discovered (available) stream, and
   subscribe to it. Reads then flow forwarder -> receiver, the receiver
   re-exposes the stream on a local TCP port, and DBF/announcer follow.

iroh is configured deterministically (relays disabled, discovery off, loopback
direct addresses, seeded keys) so it all works with no external network.

Receiver modes (``--receiver``):

* ``tauri``    (default) launch the real desktop app via ``cargo tauri dev``.
* ``headless`` launch ``receiver-headless`` (no GUI; control API + data plane).
* ``none``     set everything up and print the launch command + env, but do not
               spawn a receiver (launch it yourself).

Press Ctrl-C (or close the receiver app) to tear the whole stack down.

Usage::

    uv run scripts/dev_stack.py                 # build + launch desktop app
    uv run scripts/dev_stack.py --receiver headless
    uv run scripts/dev_stack.py --receiver none
    uv run scripts/dev_stack.py --no-build      # skip the build step
    uv run scripts/dev_stack.py --read-delay-ms 500
    uv run scripts/dev_stack.py --data-dir /tmp/rt-dev   # reuse a data dir

Stdlib only; uses ``npm``/``cargo`` to build the UIs + service binaries and
``cargo tauri`` to launch the desktop app.
"""

from __future__ import annotations

import argparse
import contextlib
import os
import shutil
import signal
import socket
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
# get distinct node ids. Loopback-only; never used outside development. Keeping
# them stable means a re-run (with a preserved server DB / --data-dir) keeps
# the same identities, so prior approvals still apply.
FORWARDER_SEED_HEX = "cd" * 32
RECEIVER_SEED_HEX = "ab" * 32

# Single dev provisioning token: the server accepts it for M2M device
# endpoints (/register, /forwarder/catalog, /allowlist/receivers, /forwarders,
# /announcer/*), and the forwarder + receiver present it as their device token.
# Production uses per-device tokens; one shared token is fine for local dev.
PROVISIONING_TOKEN = "dev-provisioning-token"

# Reads file shape (looped forever by the emulator).
NUM_FRAMES = 25

# Stable preferred ports for the two web UIs so their URLs are consistent across
# runs. Overridable via CLI flags; each falls back to an OS-assigned free port
# if the preferred one is already in use.
DEFAULT_FORWARDER_STATUS_PORT = 8787
DEFAULT_SERVER_PORT = 8675


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


def fixed_or_free_tcp_port(preferred: int, *, what: str) -> int:
    """Use ``preferred`` if it is free, else fall back to an OS-assigned port.

    Lets the dev stack expose stable UI URLs across runs while still degrading
    gracefully (instead of crashing) if the preferred port is already taken.
    """
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        try:
            s.bind(("127.0.0.1", preferred))
            return preferred
        except OSError:
            s.bind(("127.0.0.1", 0))
            fallback = s.getsockname()[1]
    print(f"[ports] {what} port {preferred} is in use; falling back to {fallback}")
    return fallback


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
    # Build both web UIs so the forwarder and server can embed them (each
    # serves its UI only when built with `--features embed-ui`, which embeds the
    # SvelteKit `build/` output at compile time).
    print("[build] web UIs (npm run build: forwarder-ui, server-ui) ...")
    for app in ("apps/forwarder-ui", "apps/server-ui"):
        subprocess.run(
            ["npm", "run", "build", "--workspace", app],
            cwd=str(REPO_ROOT),
            check=True,
        )
    print("[build] cargo build -p emulator -p receiver ...")
    subprocess.run(
        ["cargo", "build", "-p", "emulator", "-p", "receiver"],
        cwd=str(REPO_ROOT),
        check=True,
    )
    print("[build] cargo build -p forwarder -p server --features embed-ui ...")
    subprocess.run(
        ["cargo", "build", "-p", "forwarder", "--features", "embed-ui"],
        cwd=str(REPO_ROOT),
        check=True,
    )
    subprocess.run(
        ["cargo", "build", "-p", "server", "--features", "embed-ui"],
        cwd=str(REPO_ROOT),
        check=True,
    )


# ---------------------------------------------------------------------------
# Forwarder config (prod-like: server registration + allow-list fetch; NO
# static allow-list).
# ---------------------------------------------------------------------------
def write_forwarder_config(
    path: Path,
    *,
    auth_token_file: Path,
    journal_path: Path,
    status_port: int,
    emulator_port: int,
    fanout_port: int,
    p2p_port: int,
    server_url: str,
    server_token_file: Path,
) -> None:
    path.write_text(
        f"""schema_version = 1
display_name = "Dev Forwarder"

[auth]
token_file = "{auth_token_file}"

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
server_url = "{server_url}"
server_token_file = "{server_token_file}"
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
    parser.add_argument("--no-build", action="store_true", help="skip the build step")
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
    parser.add_argument(
        "--forwarder-status-port",
        type=int,
        default=DEFAULT_FORWARDER_STATUS_PORT,
        help=(
            "preferred port for the forwarder status HTTP/UI "
            f"(default: {DEFAULT_FORWARDER_STATUS_PORT}; "
            "falls back to a free port if taken)"
        ),
    )
    parser.add_argument(
        "--server-port",
        type=int,
        default=DEFAULT_SERVER_PORT,
        help=(
            "preferred port for the server HTTP/UI "
            f"(default: {DEFAULT_SERVER_PORT}; "
            "falls back to a free port if taken)"
        ),
    )
    args = parser.parse_args()

    if not args.no_build:
        cargo_build()

    work_dir = Path(tempfile.mkdtemp(prefix="rt-dev-stack-"))
    keep = args.keep or args.data_dir is not None
    receiver_data_dir = (args.data_dir or (work_dir / "receiver-data")).resolve()
    receiver_data_dir.mkdir(parents=True, exist_ok=True)

    # --- Ports (loopback only) ---
    # The two web UIs use stable preferred ports (overridable) so their URLs
    # stay consistent across runs; the rest stay ephemeral. Each preferred port
    # falls back to an OS-assigned free port if it happens to be in use.
    emulator_port = free_tcp_port()
    forwarder_status_port = fixed_or_free_tcp_port(
        args.forwarder_status_port, what="forwarder status/UI"
    )
    forwarder_fanout_port = free_tcp_port()
    forwarder_p2p_port = free_udp_port()
    server_port = fixed_or_free_tcp_port(args.server_port, what="server UI")

    server_url = f"http://127.0.0.1:{server_port}"
    receiver_id = "dev-receiver"
    # The forwarder journals each reader stream under its network address; the
    # catalog stream id the receiver will discover is therefore this:
    expected_stream_id = f"127.0.0.1:{emulator_port}"

    # --- Files ---
    reads_file = work_dir / "reads.txt"
    reads_file.write_text(
        "\n".join(build_frame(i) for i in range(1, NUM_FRAMES + 1)) + "\n"
    )
    auth_token_file = work_dir / "forwarder-auth-token"
    auth_token_file.write_text("dev-forwarder-auth-token\n")
    server_token_file = work_dir / "server-token"
    server_token_file.write_text(PROVISIONING_TOKEN + "\n")
    journal_path = work_dir / "forwarder.sqlite3"
    thin_db_path = work_dir / "server.sqlite3"
    forwarder_config = work_dir / "forwarder.toml"

    write_forwarder_config(
        forwarder_config,
        auth_token_file=auth_token_file,
        journal_path=journal_path,
        status_port=forwarder_status_port,
        emulator_port=emulator_port,
        fanout_port=forwarder_fanout_port,
        p2p_port=forwarder_p2p_port,
        server_url=server_url,
        server_token_file=server_token_file,
    )

    # The RT_* env the desktop app / receiver-headless read to start P2P. No
    # forwarder node id/addr: the receiver discovers approved forwarders from
    # the server.
    receiver_env = {
        "RT_RECEIVER_DATA_DIR": str(receiver_data_dir),
        "RT_RECEIVER_ID": receiver_id,
        "RT_P2P_SECRET_KEY_SEED_HEX": RECEIVER_SEED_HEX,
        "RT_P2P_SERVER_URL": server_url,
        "RT_P2P_SERVER_TOKEN": PROVISIONING_TOKEN,
        "RT_P2P_RECONCILE_MS": "1000",
        "RUST_LOG": os.environ.get("RUST_LOG", "info,receiver=debug"),
    }

    stack = Stack()
    try:
        # --- server (open in dev; serves the embedded UI at /) ---
        thin = stack.add(Managed(
            name="server",
            argv=[str(bin_path("server"))],
            log_path=work_dir / "server.log",
            env={
                "SERVER_DB_PATH": str(thin_db_path),
                "BIND_ADDR": f"127.0.0.1:{server_port}",
                "SERVER_PROVISIONING_TOKEN": PROVISIONING_TOKEN,
                "LOG_LEVEL": "info",
            },
        ))
        thin.start()
        wait_tcp(server_port, timeout=20, what="server")
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

        # --- forwarder (P2P, seeded; self-registers + pushes catalog + fetches
        #     the receiver allow-list from the server) ---
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
            server_url=server_url,
            server_port=server_port,
            forwarder_status_port=forwarder_status_port,
            emulator_port=emulator_port,
            expected_stream_id=expected_stream_id,
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
        argv = [
            str(bin_path("receiver-headless")),
            "--data-dir", receiver_env["RT_RECEIVER_DATA_DIR"],
            "--bind-addr", "127.0.0.1:0",
            "--receiver-id", receiver_env["RT_RECEIVER_ID"],
            "--p2p-secret-key-seed-hex", receiver_env["RT_P2P_SECRET_KEY_SEED_HEX"],
            "--p2p-server-url", receiver_env["RT_P2P_SERVER_URL"],
            "--p2p-server-token", receiver_env["RT_P2P_SERVER_TOKEN"],
            "--p2p-reconcile-ms", receiver_env["RT_P2P_RECONCILE_MS"],
        ]
        print("\n[receiver] launching receiver-headless in the foreground "
              "(Ctrl-C to stop the whole stack) ...")
        env = dict(os.environ)
        env.update(receiver_env)
        proc = subprocess.Popen(argv, cwd=str(REPO_ROOT), env=env)
        stack.procs.append(Managed(name="receiver-headless", argv=argv,
                                   log_path=work_dir / "receiver-headless.log",
                                   proc=proc))
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
    thin = kw["server_url"]
    print("\n" + "=" * 72)
    print("  Rusty Timer — local P2P dev stack is UP (prod-like flow)")
    print("=" * 72)
    print(f"  Server UI:       {thin}/         (status dashboard)")
    print(f"  Server admin:    {thin}/admin    (APPROVE devices here)")
    print(f"  Server announcer:{thin}/announcer")
    print(f"  Forwarder UI:       http://127.0.0.1:{kw['forwarder_status_port']}/   (status API at /api/v1/status)")
    print(f"  Emulator (reads):   127.0.0.1:{kw['emulator_port']}   (log: {kw['work_dir']}/emulator.log)")
    print(f"  Expected stream id: {kw['expected_stream_id']}   (appears once the forwarder is approved)")
    print(f"  Receiver data dir:  {kw['receiver_data_dir']}")
    print(f"  Logs / work dir:    {kw['work_dir']}")
    print("-" * 72)
    print("  Do this to see reads flow (the prod-like approval handshake):")
    print(f"    1. Open {thin}/admin and APPROVE both the 'forwarder' and")
    print("       'receiver' devices (give them names).")
    print("    2. In the receiver app, open the Streams tab; the discovered")
    print("       stream appears as 'Available'. Click Subscribe.")
    print("    3. Reads now flow; the Streams tab shows the local TCP output")
    print("       port your timing software can connect to. (Enable DBF in the")
    print("       receiver Admin tab if you want DBF output.)")
    print("-" * 72)
    print("  Receiver env (read by the desktop app / receiver-headless):")
    for key in (
        "RT_RECEIVER_DATA_DIR", "RT_RECEIVER_ID", "RT_P2P_SECRET_KEY_SEED_HEX",
        "RT_P2P_SERVER_URL", "RT_P2P_SERVER_TOKEN", "RT_P2P_RECONCILE_MS",
    ):
        print(f"    {key}={env[key]}")
    if kw["mode"] == "none":
        cmd = " ".join(f'{k}="{env[k]}"' for k in (
            "RT_RECEIVER_DATA_DIR", "RT_RECEIVER_ID", "RT_P2P_SECRET_KEY_SEED_HEX",
            "RT_P2P_SERVER_URL", "RT_P2P_SERVER_TOKEN", "RT_P2P_RECONCILE_MS",
        ))
        print("-" * 72)
        print("  Launch the desktop app yourself with:")
        print(f"    cd {RECEIVER_UI_DIR}")
        print(f"    {cmd} cargo tauri dev")
    print("=" * 72 + "\n")


if __name__ == "__main__":
    sys.exit(main())
