#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT_PATH="${ROOT_DIR}/sbc/rt-setup.sh"

# Source helper functions from setup script without invoking main().
source "${SCRIPT_PATH}"

# Stub external commands used by verify() to force failure path deterministically.
systemctl() { return 1; }
curl() { return 1; }
sleep() { :; }

set +e
verify >/dev/null 2>&1
status=$?
set -e

if [[ ${status} -eq 0 ]]; then
  echo "FAIL: verify should return non-zero when checks fail" >&2
  exit 1
fi

echo "PASS: verify failure path returns non-zero"
