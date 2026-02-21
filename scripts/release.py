# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///

"""
Rusty Timer Release Helper
===========================
Bumps service versions, validates release artifacts, commits/tags, and pushes.

Usage:
    uv run scripts/release.py forwarder --patch
    uv run scripts/release.py server --patch
    uv run scripts/release.py forwarder emulator --minor
    uv run scripts/release.py server --version 2.0.0 --server-local-docker-build
    uv run scripts/release.py server --version 2.0.0 --server-local-docker-build --server-docker-image iwismer/rt-server
    uv run scripts/release.py receiver --version 2.0.0
    uv run scripts/release.py forwarder --patch --dry-run
"""

import argparse
import os
import re
import shlex
import subprocess
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
VALID_SERVICES = ("forwarder", "receiver", "streamer", "emulator", "server")
EMBED_UI_SERVICES = ("forwarder", "receiver")
PACKAGE_NAME_OVERRIDES = {"emulator": "emulator-bin"}
UI_WORKSPACES = {
    "forwarder": "apps/forwarder-ui",
    "receiver": "apps/receiver-ui",
    "server": "apps/server-ui",
}
SERVER_DOCKERFILE = "services/server/Dockerfile"
DEFAULT_SERVER_DOCKER_IMAGE = "iwismer/rt-server"
VERSION_FORMAT_RE = re.compile(r"^\d+\.\d+\.\d+$")
RESET = "\x1b[0m"
STYLE_CODES = {
    "step": "\x1b[1;36m",
    "command": "\x1b[33m",
    "success": "\x1b[32m",
    "dry_run": "\x1b[2;37m",
    "header": "\x1b[1;34m",
    "warning": "\x1b[1;33m",
    "error": "\x1b[1;31m",
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Rusty Timer Release Helper — bump versions, commit, tag, and push."
    )
    parser.add_argument(
        "services",
        nargs="+",
        choices=VALID_SERVICES,
        metavar="SERVICE",
        help=f"One or more services to release: {', '.join(VALID_SERVICES)}",
    )

    bump_group = parser.add_mutually_exclusive_group(required=True)
    bump_group.add_argument("--major", action="store_true", help="Bump the major version")
    bump_group.add_argument("--minor", action="store_true", help="Bump the minor version")
    bump_group.add_argument("--patch", action="store_true", help="Bump the patch version")
    bump_group.add_argument("--version", metavar="X.Y.Z", help="Set an explicit version")

    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show release plan without making changes",
    )
    parser.add_argument(
        "--yes", "-y",
        action="store_true",
        help="Skip confirmation prompt",
    )
    parser.add_argument(
        "--server-docker-image",
        default=DEFAULT_SERVER_DOCKER_IMAGE,
        help=(
            "Docker image repository for optional local server Docker build "
            f"(default: {DEFAULT_SERVER_DOCKER_IMAGE})"
        ),
    )
    parser.add_argument(
        "--server-local-docker-build",
        action="store_true",
        help="For server releases, run a local Docker build check before commit/tag",
    )

    args = parser.parse_args()

    if args.version and not VERSION_FORMAT_RE.match(args.version):
        parser.error(f"--version must be in X.Y.Z format (got: {args.version!r})")

    return args


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    """Run a subprocess command with sensible defaults."""
    defaults = {"check": True, "capture_output": True, "text": True, "cwd": REPO_ROOT}
    defaults.update(kwargs)
    return subprocess.run(cmd, **defaults)


def supports_color() -> bool:
    if os.getenv("NO_COLOR") is not None:
        return False
    term = os.getenv("TERM", "")
    if term.lower() == "dumb":
        return False
    return sys.stdout.isatty()


def style(text: str, *, role: str, color_enabled: bool | None = None) -> str:
    enabled = supports_color() if color_enabled is None else color_enabled
    if not enabled:
        return text
    code = STYLE_CODES.get(role)
    if code is None:
        return text
    return f"{code}{text}{RESET}"


def log_command(cmd: list[str], *, execute: bool) -> None:
    print(style(f"    $ {shlex.join(cmd)}", role="command"))
    if execute:
        run(cmd, capture_output=False)
    else:
        print(style("    (dry-run) skipped", role="dry_run"))


def git_is_dirty() -> bool:
    result = run(["git", "status", "--porcelain"])
    return bool(result.stdout.strip())


def git_current_branch() -> str:
    result = run(["git", "rev-parse", "--abbrev-ref", "HEAD"])
    return result.stdout.strip()


def read_version(service: str) -> str:
    cargo_toml = REPO_ROOT / "services" / service / "Cargo.toml"
    with cargo_toml.open("rb") as f:
        data = tomllib.load(f)
    version = data.get("package", {}).get("version")
    if not version:
        print(f"Error: no version in {cargo_toml}", file=sys.stderr)
        sys.exit(1)
    return version


def bump_version(current: str, *, major: bool, minor: bool, patch: bool) -> str:
    parts = current.split(".")
    maj, min_, pat = int(parts[0]), int(parts[1]), int(parts[2])
    if major:
        return f"{maj + 1}.0.0"
    if minor:
        return f"{maj}.{min_ + 1}.0"
    if patch:
        return f"{maj}.{min_}.{pat + 1}"
    # Should not reach here
    raise ValueError("No bump type specified")


def compute_new_version(current: str, args: argparse.Namespace) -> str:
    if args.version:
        return args.version
    return bump_version(current, major=args.major, minor=args.minor, patch=args.patch)


def write_version(service: str, new_version: str) -> None:
    cargo_toml = REPO_ROOT / "services" / service / "Cargo.toml"
    text = cargo_toml.read_text()
    try:
        new_text = update_package_version(text, new_version)
    except ValueError:
        print(f"Error: failed to update version in {cargo_toml}", file=sys.stderr)
        sys.exit(1)
    cargo_toml.write_text(new_text)


def update_package_version(text: str, new_version: str) -> str:
    lines = text.splitlines(keepends=True)
    in_package = False
    for i, line in enumerate(lines):
        stripped = line.strip()
        if stripped == "[package]":
            in_package = True
            continue
        if in_package and stripped.startswith("[") and stripped.endswith("]"):
            break
        if in_package and stripped.startswith("version"):
            line_ending = ""
            if line.endswith("\r\n"):
                line_ending = "\r\n"
            elif line.endswith("\n"):
                line_ending = "\n"
            prefix = line[: len(line) - len(line.lstrip())]
            lines[i] = f'{prefix}version = "{new_version}"{line_ending}'
            return "".join(lines)
    raise ValueError("No package version found in [package] section")


def parse_semver(v: str) -> tuple[int, int, int]:
    parts = v.split(".")
    return int(parts[0]), int(parts[1]), int(parts[2])


def service_uses_embed_ui(service: str) -> bool:
    return service in EMBED_UI_SERVICES


def service_ui_workspace(service: str) -> str | None:
    return UI_WORKSPACES.get(service)


def server_image_tags(image_repo: str, version: str) -> tuple[str, str]:
    return f"{image_repo}:v{version}", f"{image_repo}:latest"


def run_release_workflow_checks(
    service: str,
    *,
    new_version: str,
    server_docker_image: str,
    server_local_docker_build: bool,
    start_step: int,
) -> int:
    step = start_step
    ui_workspace = service_ui_workspace(service)
    if ui_workspace is not None:
        print(style(f"  [{step}] Run UI checks for {ui_workspace}", role="step"))
        step += 1
        log_command(["npm", "ci"], execute=True)
        log_command(["npm", "run", "lint", "--workspace", ui_workspace], execute=True)
        log_command(["npm", "run", "check", "--workspace", ui_workspace], execute=True)
        log_command(["npm", "test", "--workspace", ui_workspace], execute=True)
        print(style("    UI checks passed", role="success"))

    if service == "server" and server_local_docker_build:
        image_version_tag, image_latest_tag = server_image_tags(server_docker_image, new_version)
        print(style(f"  [{step}] Build server Docker image", role="step"))
        step += 1
        log_command(
            [
                "docker",
                "build",
                "-t",
                image_version_tag,
                "-t",
                image_latest_tag,
                "-f",
                SERVER_DOCKERFILE,
                ".",
            ],
            execute=True,
        )
        print(
            style(
                f"    Docker build passed ({image_version_tag}, {image_latest_tag})",
                role="success",
            )
        )
        return step

    if service == "server":
        print(
            style(
                "    Skipping local server Docker build (enable with --server-local-docker-build)",
                role="warning",
            )
        )

    build_cmd = [
        "cargo",
        "build",
        "--release",
        "--package",
        PACKAGE_NAME_OVERRIDES.get(service, service),
        "--bin",
        service,
    ]
    if service_uses_embed_ui(service):
        build_cmd.extend(["--features", "embed-ui"])

    print(style(f"  [{step}] Run release build", role="step"))
    step += 1
    log_command(build_cmd, execute=True)
    print(style("    Release build passed", role="success"))
    return step


def git_head_sha() -> str:
    result = run(["git", "rev-parse", "HEAD"])
    return result.stdout.strip()


def rollback_transaction(start_head: str, created_tags: list[str]) -> None:
    for tag in reversed(created_tags):
        run(["git", "tag", "-d", tag], check=False)
    run(["git", "reset", "--hard", start_head], check=False)


def main() -> None:
    args = parse_args()
    services = list(dict.fromkeys(args.services))

    # --- Safety checks ---
    if git_is_dirty():
        print("Error: working tree is dirty. Commit or stash changes first.", file=sys.stderr)
        sys.exit(1)

    branch = git_current_branch()
    if branch != "master":
        print(f"Error: must be on the master branch (currently on: {branch})", file=sys.stderr)
        sys.exit(1)

    # --- Build release plan ---
    plan: list[tuple[str, str, str]] = []  # (service, current, new)
    skipped: list[tuple[str, str]] = []  # (service, version)

    for service in services:
        current = read_version(service)
        new = compute_new_version(current, args)
        if current == new:
            skipped.append((service, current))
        else:
            if parse_semver(new) < parse_semver(current):
                print(
                    style(
                        f"  WARNING: {service} version downgrade {current} -> {new}",
                        role="warning",
                    )
                )
            plan.append((service, current, new))

    # --- Display plan ---
    print()
    print(style("Release Plan", role="header"))
    print(style("=" * 50, role="header"))
    for service, current, new in plan:
        print(f"  {service}: {current} -> {new}")
    for service, version in skipped:
        print(f"  {service}: {version} (already at target, skipping)")
    print()

    if not plan:
        print("Nothing to release — all services are already at the target version.")
        return

    if args.dry_run:
        print(
            style(
                "Dry run mode: checks/builds will run, mutating steps are printed only.",
                role="dry_run",
            )
        )
    else:
        start_head = git_head_sha()

    # --- Confirm ---
    if not args.yes:
        answer = input("Proceed? [y/N] ").strip().lower()
        if answer not in ("y", "yes"):
            print("Aborted.")
            sys.exit(0)

    # --- Execute ---
    tags: list[str] = []
    try:
        for service, current, new in plan:
            print(f"\n--- {service}: {current} -> {new} ---")
            cargo_path = f"services/{service}/Cargo.toml"
            step = 1

            # Update Cargo.toml
            print(style(f"  [{step}] Update {cargo_path} version to {new}", role="step"))
            step += 1
            if args.dry_run:
                print(style(f"    (dry-run) would update {cargo_path}", role="dry_run"))
            else:
                write_version(service, new)
                print(style(f"    Updated {cargo_path}", role="success"))

            # Validate with the same checks/build used by release workflow.
            step = run_release_workflow_checks(
                service,
                new_version=new,
                server_docker_image=args.server_docker_image,
                server_local_docker_build=args.server_local_docker_build,
                start_step=step,
            )

            # Stage, commit, tag
            print(style(f"  [{step}] Stage release files", role="step"))
            step += 1
            log_command(
                ["git", "add", cargo_path, "Cargo.lock"],
                execute=not args.dry_run,
            )

            commit_msg = f"chore({service}): bump version to {new}"
            print(style(f"  [{step}] Create release commit", role="step"))
            step += 1
            log_command(
                ["git", "commit", "-m", commit_msg],
                execute=not args.dry_run,
            )
            if args.dry_run:
                print(style(f"    (dry-run) would commit: {commit_msg}", role="dry_run"))
            else:
                print(style(f"    Committed: {commit_msg}", role="success"))

            tag = f"{service}-v{new}"
            print(style(f"  [{step}] Create release tag", role="step"))
            step += 1
            log_command(["git", "tag", tag], execute=not args.dry_run)
            if args.dry_run:
                print(style(f"    (dry-run) would tag: {tag}", role="dry_run"))
            else:
                print(style(f"    Tagged: {tag}", role="success"))
                tags.append(tag)

        # --- Push ---
        print(style("\n[Final Step] Push commits and tags", role="step"))
        push_cmd = ["git", "push", "--atomic", "origin", "master", *tags]
        if args.dry_run and plan:
            dry_tags = [f"{service}-v{new}" for service, _, new in plan]
            push_cmd = ["git", "push", "--atomic", "origin", "master", *dry_tags]
        log_command(push_cmd, execute=not args.dry_run)

        if args.dry_run:
            print(style("Dry run complete.", role="dry_run"))
        else:
            print(style("Done!", role="success"))
    except subprocess.CalledProcessError as e:
        if args.dry_run:
            print(
                style("Error: dry-run checks failed.", role="error"),
                file=sys.stderr,
            )
        else:
            print(
                style("Error: release failed, rolling back transaction.", role="error"),
                file=sys.stderr,
            )
        if e.stderr:
            print(e.stderr, file=sys.stderr)
        if not args.dry_run:
            rollback_transaction(start_head, tags)
        sys.exit(1)


if __name__ == "__main__":
    main()
