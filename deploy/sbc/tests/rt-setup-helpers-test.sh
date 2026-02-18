#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SCRIPT_PATH="${ROOT_DIR}/sbc/rt-setup.sh"

# Source helper functions from the setup script. This should not execute main().
source "${SCRIPT_PATH}"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_eq() {
  local expected="$1"
  local actual="$2"
  local msg="$3"
  if [[ "${expected}" != "${actual}" ]]; then
    fail "${msg} (expected='${expected}' actual='${actual}')"
  fi
}

assert_nonempty() {
  local actual="$1"
  local msg="$2"
  if [[ -z "${actual}" ]]; then
    fail "${msg} (got empty string)"
  fi
}

# --- release selection across pages ---
page1='[{"tag_name":"server-v1.0.0","published_at":"2026-02-01T00:00:00Z","draft":false,"prerelease":false,"assets":[]}]'
page2='[{"tag_name":"forwarder-v1.2.3","published_at":"2026-02-10T00:00:00Z","draft":false,"prerelease":false,"assets":[{"name":"forwarder-v1.2.3-linux-arm64.tar.gz","browser_download_url":"https://example.com/fwd.tar.gz"}]}]'

url="$(select_latest_forwarder_asset_from_pages "${page1}" "${page2}")"
assert_nonempty "${url}" "release URL should be found across multiple pages"
assert_eq "https://example.com/fwd.tar.gz" "${url}" "release URL should match expected arm64 asset"

# --- probe URL derivation from bind ---
assert_eq "http://localhost:8080/healthz" "$(status_probe_url_from_bind '0.0.0.0:8080')" "wildcard bind should map to localhost"
assert_eq "http://127.0.0.1:9090/healthz" "$(status_probe_url_from_bind '127.0.0.1:9090')" "loopback bind should be preserved"

echo "PASS: rt-setup helper tests"
