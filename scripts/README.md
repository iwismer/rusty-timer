# Scripts Guide

## `dev.py` (Rusty Timer Dev Launcher)

`dev.py` sets up and launches a full local Rusty Timer dev stack in one command:
- Postgres (Docker)
- Server
- One or more emulators
- Forwarder
- Receiver

It also prepares dev auth tokens, writes runtime config, optionally uploads race data,
and opens services in `tmux` (preferred) or iTerm2 panes.

## Prerequisites

- Run from the repository root.
- Python 3.11+ via `uv run`.
- Installed tools: `docker`, `cargo`, `npm`, `curl`.
- A multiplexer:
  - `tmux` (preferred), or
  - iTerm2 with Python API enabled.

## Usage

```bash
uv run scripts/dev.py [--no-build] [--clear] [--emulator SPEC ...] [--bibchip PATH] [--ppl PATH]
```

Examples:

```bash
# Full setup + launch with one default emulator (port 10001)
uv run scripts/dev.py

# Reuse prior builds and just set up/launch runtime pieces
uv run scripts/dev.py --no-build

# Tear everything down
uv run scripts/dev.py --clear

# Single emulator with custom settings
uv run scripts/dev.py --emulator port=10001,delay=500,file=test_assets/reads.txt,type=raw

# Multiple emulators
uv run scripts/dev.py --emulator port=10001 --emulator port=10002,delay=500,type=fsls

# Auto-generate emulator reads from bibchip and upload race files
uv run scripts/dev.py --bibchip test_assets/bibchip/large.txt --ppl test_assets/ppl/large.ppl
```

## Flags

- `--no-build`: skip dashboard + Rust build steps.
- `--clear`: remove dev artifacts and exit.
- `--emulator SPEC`: add an emulator instance. Repeat this flag for multiple emulators.
- `--bibchip PATH`: upload chip file to a new race after startup; can also generate emulator reads.
- `--ppl PATH`: upload participant file to a new race after startup.

`--emulator` format:

```text
port=N,delay=MS,file=PATH,type=raw|fsls
```

- `port` is required.
- `delay` defaults to `2000`.
- `type` defaults to `raw`.
- `file` is optional.
- If no `--emulator` is provided, default is one emulator on port `10001`.

## Startup Workflow

When run normally (`--clear` not set), `dev.py` does the following:

1. Validates CLI/file inputs and port-collision rules.
2. Detects any existing dev instance and prompts whether to kill/reuse/cancel.
3. Starts or reuses Docker Postgres container `rt-postgres`.
4. Waits for Postgres readiness (`pg_isready`).
5. Applies SQL migrations from `services/server/migrations/`.
6. Writes temporary dev config/token files under `/tmp/rusty-timer-dev`.
7. Seeds forwarder/receiver dev tokens into `device_tokens`.
8. Runs `npm install` in workspace root.
9. Builds dashboard (`apps/server-ui`) unless `--no-build`.
10. Builds Rust binaries unless `--no-build`.
11. Launches panes in `tmux` (or iTerm2 fallback).
12. Auto-configures receiver profile/connect over control API.
13. Optionally creates a race and uploads bibchip/PPL files.

## Generated Dev Files

Created under `/tmp/rusty-timer-dev`:

- `forwarder.toml`
- `forwarder-token.txt`
- `receiver-token.txt`
- `configure-receiver.sh`
- `forwarder.sqlite3` (forwarder journal)
- `race-setup.log` (if race upload requested)
- `iterm-window-id.txt` (when launched via iTerm2)

Default dev tokens:
- Forwarder token: `rusty-dev-forwarder`
- Receiver token: `rusty-dev-receiver`

## Runtime Notes

- Server starts on `http://127.0.0.1:8080`.
- Receiver control API is expected at `http://127.0.0.1:9090`.
- If `apps/server-ui/build` exists, server is launched with `DASHBOARD_DIR` set to that path.
- On startup, the script validates collisions across:
  - emulator ports
  - forwarder fallback ports (`emulator_port + 1000`)
  - receiver-derived default local ports

If these collide, startup stops with an error.

## Bibchip/PPL Behavior

- `--bibchip` and `--ppl` files must exist or startup exits early.
- If `--bibchip` is set and the first emulator has no explicit `file=...`, the script generates
  emulator-compatible reads at `/tmp/rusty-timer-dev/generated-reads.txt` and wires that file
  into the first emulator.
- Race setup runs in the background after server health is ready:
  - creates race `Dev Race`
  - uploads bibchip to `/api/v1/races/{race_id}/chips/upload`
  - uploads PPL to `/api/v1/races/{race_id}/participants/upload`

## Existing Instance Detection

Before setup, the script checks for:
- tmux session `rusty-dev`
- listeners on server port `8080`

If it detects a prior dev instance, it prompts to kill/restart, continue, or cancel.
For non-dev processes using port `8080`, it refuses to kill them automatically.

## Cleanup

Use:

```bash
uv run scripts/dev.py --clear
```

This attempts to:
- kill tmux session `rusty-dev`
- remove Docker container `rt-postgres`
- delete `/tmp/rusty-timer-dev`

## `release.py` (Rusty Timer Release Helper)

`release.py` automates binary-service releases by bumping service versions, validating builds, creating commits/tags, and pushing everything atomically.

It is intended for these services only:
- `forwarder`
- `receiver`
- `streamer`
- `emulator`

`server` is intentionally excluded (it is deployed via Docker).

## Prerequisites

- Run from a clean git working tree.
- Be on the `master` branch.
- Have push access to `origin/master`.
- Have Rust toolchain available (`cargo build --release` is run per service).
- For `forwarder`/`receiver` releases, have Node.js + npm available (UI lint/check/test run).
- Use `uv` to run the script in this repository.

## Usage

```bash
uv run scripts/release.py SERVICE [SERVICE ...] (--major | --minor | --patch | --version X.Y.Z) [--dry-run] [--yes]
```

Examples:

```bash
# Patch release for one service
uv run scripts/release.py forwarder --patch

# Minor release for multiple services in one transaction
uv run scripts/release.py forwarder emulator --minor

# Set an explicit version
uv run scripts/release.py receiver --version 2.0.0

# Preview only (no file or git changes)
uv run scripts/release.py forwarder --patch --dry-run
```

## Flags

- `--major`: bump `X.Y.Z` to `X+1.0.0`
- `--minor`: bump `X.Y.Z` to `X.Y+1.0`
- `--patch`: bump `X.Y.Z` to `X.Y.Z+1`
- `--version X.Y.Z`: set an exact semantic version (must match `^\d+\.\d+\.\d+$`)
- `--dry-run`: run checks/builds, print mutating commands, and skip file/git mutations
- `--yes`, `-y`: skip interactive confirmation prompt

## What the Script Does

For each requested service, the script:
1. Reads `services/<service>/Cargo.toml` package version.
2. Computes target version.
3. Skips services already at target.
4. Updates `services/<service>/Cargo.toml`.
5. Runs release-workflow parity checks/build:
   - `forwarder`/`receiver`: `npm ci`, UI `lint`, UI `check`, UI tests for `apps/<service>-ui`
   - all services: `cargo build --release --package <service> --bin <service>` (`--features embed-ui` for `forwarder`/`receiver`)
6. Stages `services/<service>/Cargo.toml` and `Cargo.lock`.
7. Creates commit: `chore(<service>): bump version to <new_version>`.
8. Creates tag: `<service>-v<new_version>`.

The script prints each step and the exact command before execution.
In `--dry-run`, it still runs the checks/build commands, but prints and skips
mutating commands (version file write, `git add`, `git commit`, `git tag`,
`git push`).
When output is a TTY, step/command/status lines are colorized for readability.
Set `NO_COLOR=1` to force plain text output.

After all services succeed, it pushes branch + tags in a single atomic command:

```bash
git push --atomic origin master <tag1> <tag2> ...
```

## Safety and Failure Behavior

- Fails fast if the working tree is dirty.
- Fails fast if current branch is not `master`.
- Prints the full release plan before execution.
- Warns on explicit version downgrades (`new < current`).
- Uses transactional rollback on failure:
  - Deletes any tags created in this run.
  - Resets git state back to starting `HEAD`.

## Operational Notes

- Duplicate service names in CLI args are de-duplicated (first occurrence wins).
- If every selected service is already at the target version, it exits with “Nothing to release”.
- Because rollback uses `git reset --hard`, only run this script when your tree is clean (the script enforces this).

## `sbc_cloud_init.py` (SBC Cloud-Init File Wizard)

`sbc_cloud_init.py` asks deployment questions and generates the two files needed
for Raspberry Pi cloud-init setup:

- `user-data`
- `network-config`

Use it when preparing an SBC image so you do not need to manually edit YAML.

### Usage

```bash
uv run scripts/sbc_cloud_init.py
```

Optional output directory:

```bash
uv run scripts/sbc_cloud_init.py --output-dir /tmp/sbc-config
```

Enable full first-boot automation (no SSH setup commands required):

```bash
uv run scripts/sbc_cloud_init.py --auto-first-boot
```

In `--auto-first-boot` mode, the wizard also asks for:
- Server base URL
- Forwarder auth token
- Reader targets
- Status bind address

and writes a `user-data` that runs `deploy/sbc/rt-setup.sh` non-interactively
on first boot.
The generated setup env also sets forwarder `display_name` to the same value as
the configured hostname.

The script prompts for:

- Hostname
- SSH admin username
- SSH public key
- Static IPv4/CIDR for eth0
- Default gateway
- DNS servers
- Optional Wi-Fi settings (SSID/password/regulatory domain for `wlan0`)

By default, generated files are written to `deploy/sbc/generated/`.
