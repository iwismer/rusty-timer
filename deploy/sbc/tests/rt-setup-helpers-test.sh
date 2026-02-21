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

assert_contains() {
  local haystack="$1"
  local needle="$2"
  local msg="$3"
  if [[ "${haystack}" != *"${needle}"* ]]; then
    fail "${msg} (missing='${needle}')"
  fi
}

# --- release selection across pages ---
page1='[{"tag_name":"server-v1.0.0","published_at":"2026-02-01T00:00:00Z","draft":false,"prerelease":false,"assets":[]}]'
page2='[{"tag_name":"forwarder-v1.2.3","published_at":"2026-02-10T00:00:00Z","draft":false,"prerelease":false,"assets":[{"name":"forwarder-v1.2.3-aarch64-unknown-linux-gnu.tar.gz","browser_download_url":"https://example.com/fwd.tar.gz"}]}]'

url="$(select_latest_forwarder_asset_from_pages "${page1}" "${page2}")"
assert_nonempty "${url}" "release URL should be found across multiple pages"
assert_eq "https://example.com/fwd.tar.gz" "${url}" "release URL should match expected arm64 asset"

# --- probe URL derivation from bind ---
assert_eq "http://localhost:8080/healthz" "$(status_probe_url_from_bind '0.0.0.0:8080')" "wildcard ipv4 bind should map to localhost"
assert_eq "http://localhost:6060/healthz" "$(status_probe_url_from_bind '[::]:6060')" "wildcard ipv6 bind should map to localhost"
assert_eq "http://127.0.0.1:9090/healthz" "$(status_probe_url_from_bind '127.0.0.1:9090')" "explicit ipv4 bind should preserve probe host"
assert_eq "http://192.168.1.50:8080/healthz" "$(status_probe_url_from_bind '192.168.1.50:8080')" "specific ipv4 bind should preserve probe host"
assert_eq "http://[::1]:7070/healthz" "$(status_probe_url_from_bind '[::1]:7070')" "explicit ipv6 bind should preserve probe host"

# --- checksum extraction helper ---
checksums=$'aaaaaaaa  forwarder-v1.2.3-aarch64-unknown-linux-gnu.tar.gz\nbbbbbbbb  forwarder-v1.2.3-linux-x86_64.tar.gz\n'
assert_eq "aaaaaaaa" "$(checksum_for_asset_from_sha256sums "${checksums}" "forwarder-v1.2.3-aarch64-unknown-linux-gnu.tar.gz")" "should pick checksum for requested asset"
assert_eq "" "$(checksum_for_asset_from_sha256sums "${checksums}" "forwarder-v1.2.3-linux-armv7.tar.gz")" "should return empty when asset missing"

# --- verify policy helper ---
assert_eq "skip_verify" "$(install_verify_policy yes n)" "active service + no restart should skip verify"
assert_eq "run_verify" "$(install_verify_policy yes y)" "active service + yes restart should run verify"
assert_eq "run_verify" "$(install_verify_policy no '')" "inactive service should run verify"

# --- non-interactive env helpers ---
assert_eq "0" "$(bool_env_is_true '')" "empty env should be false"
assert_eq "1" "$(bool_env_is_true '1')" "1 should be true"
assert_eq "1" "$(bool_env_is_true 'yes')" "yes should be true"
assert_eq "0" "$(bool_env_is_true 'no')" "no should be false"

unset RT_SETUP_ALLOW_POWER_ACTIONS
assert_eq "true" "$(allow_power_actions_toml_value)" "power actions should default to true for SBC setup"
RT_SETUP_ALLOW_POWER_ACTIONS=0
assert_eq "false" "$(allow_power_actions_toml_value)" "RT_SETUP_ALLOW_POWER_ACTIONS=0 should disable power actions"
RT_SETUP_ALLOW_POWER_ACTIONS=true
assert_eq "true" "$(allow_power_actions_toml_value)" "RT_SETUP_ALLOW_POWER_ACTIONS=true should enable power actions"
unset RT_SETUP_ALLOW_POWER_ACTIONS

targets="$(reader_targets_from_env $'192.168.1.10:10000\n192.168.1.11:10000')"
assert_eq $'192.168.1.10:10000\n192.168.1.11:10000' "${targets}" "newline reader list should be preserved"

targets="$(reader_targets_from_env '192.168.1.10:10000, 192.168.1.11:10000')"
assert_eq $'192.168.1.10:10000\n192.168.1.11:10000' "${targets}" "comma reader list should normalize to newline list"

# --- hostname/display name + TOML escaping helpers ---
host_name="$(default_forwarder_display_name)"
assert_nonempty "${host_name}" "default display name should resolve to non-empty host name"
assert_eq "a\\\"b\\\\c" "$(toml_escape_string 'a"b\c')" "toml_escape_string should escape quotes and backslashes"

# --- service unit and staged-update helper rendering ---
unit="$(render_forwarder_systemd_unit)"
assert_contains "${unit}" "User=rt-forwarder" "unit should keep service user"
assert_contains "${unit}" "PermissionsStartOnly=true" "unit should allow root pre-start hook"
assert_contains "${unit}" "ExecStartPre=/usr/local/lib/rt-forwarder-apply-staged.sh" "unit should include staged update hook"
assert_contains "${unit}" "Environment=RT_FORWARDER_UPDATE_APPLY_VIA_RESTART=1" "unit should enable restart-based update apply mode"
assert_contains "${unit}" "AmbientCapabilities=CAP_NET_BIND_SERVICE" "unit should allow binding to privileged ports"
assert_contains "${unit}" "ExecStart=/usr/local/bin/rt-forwarder" "unit should run forwarder binary"

apply_script="$(render_apply_staged_script)"
assert_contains "${apply_script}" "STAGED_PATH=\"/var/lib/rusty-timer/.forwarder-staged\"" "apply helper should use staged path"
assert_contains "${apply_script}" "TARGET_PATH=\"/usr/local/bin/rt-forwarder\"" "apply helper should use forwarder install path"
assert_contains "${apply_script}" "mv \"\${tmp_target}\" \"\${TARGET_PATH}\"" "apply helper should atomically promote binary"
assert_contains "${apply_script}" "rm -f \"\${STAGED_PATH}\"" "apply helper should clean staged file"

sudoers="$(render_power_actions_sudoers)"
assert_contains "${sudoers}" "rt-forwarder" "sudoers should target the rt-forwarder user"
assert_contains "${sudoers}" "systemctl --no-ask-password reboot" "sudoers should allow reboot action"
assert_contains "${sudoers}" "systemctl --no-ask-password poweroff" "sudoers should allow poweroff action"

echo "PASS: rt-setup helper tests"
