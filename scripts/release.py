# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///

"""
Rusty Timer Release Helper
===========================
Bumps versions in service Cargo.toml files, commits, tags, and pushes.

Usage:
    uv run scripts/release.py forwarder --patch
    uv run scripts/release.py forwarder emulator --minor
    uv run scripts/release.py receiver --version 2.0.0
    uv run scripts/release.py forwarder --patch --dry-run
"""

import argparse
import re
import subprocess
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
# Server is excluded — it's deployed via Docker, not as a standalone binary.
VALID_SERVICES = ("forwarder", "receiver", "streamer", "emulator")
VERSION_FORMAT_RE = re.compile(r"^\d+\.\d+\.\d+$")


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

    args = parser.parse_args()

    if args.version and not VERSION_FORMAT_RE.match(args.version):
        parser.error(f"--version must be in X.Y.Z format (got: {args.version!r})")

    return args


def run(cmd: list[str], **kwargs) -> subprocess.CompletedProcess:
    """Run a subprocess command with sensible defaults."""
    defaults = {"check": True, "capture_output": True, "text": True, "cwd": REPO_ROOT}
    defaults.update(kwargs)
    return subprocess.run(cmd, **defaults)


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
                print(f"  WARNING: {service} version downgrade {current} -> {new}")
            plan.append((service, current, new))

    # --- Display plan ---
    print()
    print("Release Plan")
    print("=" * 50)
    for service, current, new in plan:
        print(f"  {service}: {current} -> {new}")
    for service, version in skipped:
        print(f"  {service}: {version} (already at target, skipping)")
    print()

    if not plan:
        print("Nothing to release — all services are already at the target version.")
        return

    if args.dry_run:
        print("Dry run — no changes made.")
        return

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

            # Update Cargo.toml
            write_version(service, new)
            print(f"  Updated {cargo_path}")

            # Validate with cargo check
            print(f"  Running cargo check -p {service}...")
            run(["cargo", "check", "-p", service], capture_output=False)
            print(f"  cargo check passed")

            # Stage, commit, tag
            run(["git", "add", cargo_path])
            commit_msg = f"chore({service}): bump version to {new}"
            run(["git", "commit", "-m", commit_msg])
            print(f"  Committed: {commit_msg}")

            tag = f"{service}-v{new}"
            run(["git", "tag", tag])
            print(f"  Tagged: {tag}")
            tags.append(tag)

        # --- Push ---
        print("\nPushing commits and tags...")
        run(["git", "push", "--atomic", "origin", "master", *tags])
        print("Done!")
    except subprocess.CalledProcessError as e:
        print("Error: release failed, rolling back transaction.", file=sys.stderr)
        if e.stderr:
            print(e.stderr, file=sys.stderr)
        rollback_transaction(start_head, tags)
        sys.exit(1)


if __name__ == "__main__":
    main()
