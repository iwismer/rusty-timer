#!/usr/bin/env python3
"""Run the deterministic E2E stack with exploratory bridge-agent artifacts."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
DEFAULT_SCENARIO = Path(__file__).with_name("bridge_goal.json")
RUN_STACK = REPO_ROOT / "scripts" / "e2e" / "run_stack.py"
REQUIRED_ARTIFACTS = ("goal.md", "transcript.md", "state.json", "findings.json")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifacts-dir", required=True, type=Path)
    parser.add_argument("--scenario", type=Path, default=DEFAULT_SCENARIO)
    parser.add_argument("--no-build", action="store_true", help="forward --no-build to run_stack.py")
    parser.add_argument("--keep", action="store_true", help="forward --keep to run_stack.py")
    args = parser.parse_args()

    cmd = [
        sys.executable,
        str(RUN_STACK),
        "--agent-ui-scenario",
        str(args.scenario),
        "--agent-ui-artifacts-dir",
        str(args.artifacts_dir),
    ]
    if args.no_build:
        cmd.append("--no-build")
    if args.keep:
        cmd.append("--keep")

    completed = subprocess.run(cmd, cwd=str(REPO_ROOT), check=False)
    if completed.returncode != 0:
        return completed.returncode

    missing = [name for name in REQUIRED_ARTIFACTS if not (args.artifacts_dir / name).exists()]
    if missing:
        print(f"missing agent artifacts: {', '.join(missing)}", file=sys.stderr)
        return 2

    findings = json.loads((args.artifacts_dir / "findings.json").read_text(encoding="utf-8"))
    if not findings.get("completed"):
        print("bridge agent did not complete; see findings.json", file=sys.stderr)
        return 2

    print(f"bridge agent artifacts: {args.artifacts_dir}")
    print(f"bridge agent judgment passed: {str(bool(findings.get('passed'))).lower()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
