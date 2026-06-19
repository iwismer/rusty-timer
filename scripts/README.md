# Scripts Guide

## Deterministic P2P E2E Stack

Use `scripts/e2e/run_stack.py` for local data-plane verification. It starts
real OS processes for the emulator, forwarder, receiver-headless, and server
with iroh configured for deterministic loopback operation: relays disabled,
discovery disabled, seeded keys, and injected local addresses.

```bash
uv run scripts/e2e/run_stack.py
```

Common options:

```bash
# Skip rebuilding binaries when target/debug is already current
uv run scripts/e2e/run_stack.py --no-build

# Keep the temporary run directory for inspection
uv run scripts/e2e/run_stack.py --keep

# Run a single SIGKILL+resume lane instead of both default lanes
uv run scripts/e2e/run_stack.py --power-loss-target receiver
uv run scripts/e2e/run_stack.py --power-loss-target forwarder

# Emit optional UI-agent diagnostic artifacts on the final lane
uv run scripts/e2e/run_stack.py \
  --agent-ui-scenario scripts/e2e/agent_ui/bridge_goal.json \
  --agent-ui-artifacts-dir /tmp/rt-agent-ui-artifacts
```

The stack writes temporary configs, SQLite databases, DBF output, logs, and UI
agent artifacts under its run directory. The backend assertions are the hard
gate: received event counts, DBF rows, TCP proxy replay, receiver cursors, and
server announcer/status state.

## Manual Dev Stack

`scripts/dev_stack.py` boots the whole loopback stack and leaves it running for
manual exploration (no assertions, no failure-injection lanes), the prod-like
way. It starts the emulator, forwarder, and server (both the forwarder and
server serve their embedded web UIs) and launches the receiver:

```bash
uv run scripts/dev_stack.py                    # build + launch the desktop app
uv run scripts/dev_stack.py --receiver headless
uv run scripts/dev_stack.py --receiver none
uv run scripts/dev_stack.py --no-build
```

There is no static allow-list or preseeded subscription: the forwarder and
receiver self-register with the server, you approve both in the server
admin UI (`/admin`), the receiver discovers the approved forwarder, and you
subscribe to the discovered stream in the receiver UI. The startup banner prints
all the URLs. The desktop app reads its server config from the `RT_P2P_*` /
`RT_RECEIVER_*` environment variables the script sets. Press Ctrl-C to tear the
stack down.

## SBC First-Boot Setup

Use the Server UI `SBC Setup` tab to provision Raspberry Pi forwarders without
hand-writing cloud-init files:

1. Open the Server UI and go to `SBC Setup`.
2. Generate a forwarder token, or add a manual token if you need a pre-shared
   value.
3. Copy the one-time token into the setup form. Generated token secrets are
   shown once; the token list only shows metadata.
4. Fill SSH identity, static Ethernet, optional Wi-Fi, server URL, reader
   target, and display-name fields.
5. Download `user-data` and `network-config`.
6. Copy both files to the Raspberry Pi boot partition and boot the SBC.
7. After the forwarder registers, approve it in the Server UI `Admin` tab.

Revoking an unused token prevents first registration. Revoking a used token also
blocks future per-device forwarder requests that authenticate with that token.
Revocation does not delete existing forwarder status or approval records.

## Agent UI Harness

`scripts/e2e/agent_ui/run_bridge_goal.py` drives the receiver-headless
`test-bridge` when that feature is enabled. It is a diagnostic lane only; the
script emits screenshots/transcripts/findings, but deterministic backend
assertions decide pass/fail.

## Release Helper

`release.py` automates service releases by bumping versions, validating release
artifacts, creating commits/tags, and pushing the branch plus each tag
separately so GitHub runs one workflow per tag.

It supports these services:

- `forwarder`
- `receiver`
- `streamer`
- `emulator`
- `server`

### Prerequisites

- Run from a clean git working tree.
- Be on the `main` branch.
- Have push access to `origin/main`.
- Have Rust available (`cargo build --release` is run per service).
- For `forwarder`/`receiver` releases, have Node.js + npm available for UI
  lint/check/test.
- Use `uv` to run the script in this repository.

### Usage

```bash
uv run scripts/release.py SERVICE [SERVICE ...] (--major | --minor | --patch | --version X.Y.Z) [--dry-run] [--yes]
```

Examples:

```bash
uv run scripts/release.py forwarder --patch
uv run scripts/release.py forwarder emulator --minor
uv run scripts/release.py receiver --version 2.0.0
uv run scripts/release.py server --patch
uv run scripts/release.py forwarder --patch --dry-run
```

### What the Script Does

For each requested service, the script:

1. Reads `services/<service>/Cargo.toml` package version.
2. Computes the target version.
3. Updates `services/<service>/Cargo.toml`. For `receiver`, it also updates
   `apps/receiver-ui/src-tauri/tauri.conf.json`.
4. Runs release-workflow parity checks:
   - `forwarder`: `npm ci`, forwarder UI lint/check/test, then release build
     with `embed-ui,eink`.
   - `receiver`: `npm ci`, receiver UI lint/check/test, then release build.
   - `streamer`, `emulator`, `server`: release build for the service
     binary.
5. Stages changed version files.
6. Creates commit `chore(<service>): bump version to <new_version>`.
7. Creates tag `<service>-v<new_version>`.
8. Pushes the branch and each tag separately.

The GitHub release workflow publishes arm64 Linux artifacts for Linux SBCs.
`server` releases are arm64-only.

## Packaging Validation

Run:

```bash
bash scripts/validate-packaging.sh
```

The validator checks that cutover-only artifacts remain: no legacy central
service paths, no legacy wire contract, forwarder Dockerfile health,
runbook coverage, release workflow routing, and executable script permissions.
