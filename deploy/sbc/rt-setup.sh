#!/usr/bin/env bash
# deploy/sbc/rt-setup.sh
#
# Interactive setup wizard for rt-forwarder on a Raspberry Pi.
#
# Downloads the forwarder binary from GitHub Releases, prompts for
# configuration values, generates forwarder.toml and the auth token
# file, installs the systemd service, and verifies it starts.
#
# Usage:
#   sudo bash rt-setup.sh

set -euo pipefail

# ── Variables ────────────────────────────────────────────────────────
GITHUB_REPO="iwismer/rusty-timer"
INSTALL_DIR="/usr/local/bin"
HELPER_DIR="/usr/local/lib"
CONFIG_DIR="/etc/rusty-timer"
DATA_DIR="/var/lib/rusty-timer"
SERVICE_USER="rt-forwarder"
STATUS_BIND="0.0.0.0:80"
VERIFY_POLICY="run_verify"
FORWARDER_BIN_PATH="${INSTALL_DIR}/rt-forwarder"
STAGED_FORWARDER_PATH="${DATA_DIR}/.forwarder-staged"
APPLY_STAGED_HELPER="${HELPER_DIR}/rt-forwarder-apply-staged.sh"
POWER_ACTIONS_SUDOERS_PATH="/etc/sudoers.d/90-rt-forwarder-power-actions"

# ── Helpers ──────────────────────────────────────────────────────────

bool_env_is_true() {
  local raw="${1:-}"
  local lower
  lower="$(printf '%s' "${raw}" | tr '[:upper:]' '[:lower:]')"
  case "${lower}" in
    1|true|yes|y|on)
      printf '1\n'
      ;;
    *)
      printf '0\n'
      ;;
  esac
}

allow_power_actions_toml_value() {
  local raw="${RT_SETUP_ALLOW_POWER_ACTIONS:-1}"
  if [[ "$(bool_env_is_true "${raw}")" == "1" ]]; then
    printf 'true\n'
  else
    printf 'false\n'
  fi
}

is_noninteractive_mode() {
  [[ "$(bool_env_is_true "${RT_SETUP_NONINTERACTIVE:-0}")" == "1" ]]
}

reader_targets_from_env() {
  local raw="${1:-}"
  if [[ -z "${raw}" ]]; then
    return 0
  fi

  printf '%s\n' "${raw}" \
    | tr ',;' '\n' \
    | sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' \
    | awk 'NF > 0'
}

is_valid_reader_target() {
  local target="$1"
  [[ "${target}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+:[0-9]+$ ]] \
    || [[ "${target}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+-[0-9]+:[0-9]+$ ]]
}

default_forwarder_display_name() {
  local current_host=""
  current_host="$(hostname -s 2>/dev/null || true)"
  if [[ -z "${current_host}" ]]; then
    current_host="$(hostname 2>/dev/null || true)"
  fi
  if [[ -z "${current_host}" ]]; then
    current_host="rt-forwarder"
  fi
  printf '%s\n' "${current_host}"
}

toml_escape_string() {
  printf '%s' "${1:-}" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

write_done_marker_if_requested() {
  local marker="${RT_SETUP_DONE_MARKER:-}"
  if [[ -z "${marker}" ]]; then
    return 0
  fi

  install -d -m 0755 "$(dirname "${marker}")"
  touch "${marker}"
  chmod 0600 "${marker}" || true
  echo "Wrote setup completion marker: ${marker}"
}

select_latest_forwarder_asset_from_pages() {
  if [[ $# -eq 0 ]]; then
    return 0
  fi

  printf '%s\n' "$@" | jq -rs '
    [
      .[]
      | if type == "array" then .[] else empty end
      | select((.tag_name // "") | startswith("forwarder-v"))
      | select((.draft // false) | not)
      | select((.prerelease // false) | not)
    ]
    | sort_by(.published_at // "")
    | reverse
    | .[]
    | .assets[]?
    | select((.name // "") | test("forwarder-.*-aarch64-unknown-linux-gnu\\.tar\\.gz$"))
    | .browser_download_url
    | select(type == "string" and length > 0)
  ' | head -n 1
}

status_probe_url_from_bind() {
  local bind="$1"
  local host="localhost"
  local port="80"

  if [[ "${bind}" =~ ^\[([0-9A-Fa-f:]+)\]:([0-9]+)$ ]]; then
    local ipv6_host="${BASH_REMATCH[1]}"
    port="${BASH_REMATCH[2]}"
    if [[ "${ipv6_host}" != "::" ]]; then
      host="[${ipv6_host}]"
    fi
  elif [[ "${bind}" =~ ^([^:]+):([0-9]+)$ ]]; then
    local ipv4_host="${BASH_REMATCH[1]}"
    port="${BASH_REMATCH[2]}"
    if [[ "${ipv4_host}" != "0.0.0.0" ]]; then
      host="${ipv4_host}"
    fi
  fi

  printf 'http://%s:%s/healthz' "${host}" "${port}"
}

checksum_for_asset_from_sha256sums() {
  local checksums="$1"
  local asset_name="$2"

  if [[ -z "${checksums}" || -z "${asset_name}" ]]; then
    return 0
  fi

  printf '%s\n' "${checksums}" | awk -v asset="${asset_name}" '
    NF >= 2 {
      hash = $1
      file = $2
      sub(/^\*/, "", file)
      sub(/^\.?\//, "", file)
      if (file == asset) {
        print hash
        exit
      }
    }
  '
}

render_apply_staged_script() {
  cat <<EOF
#!/usr/bin/env bash
set -euo pipefail

STAGED_PATH="${STAGED_FORWARDER_PATH}"
TARGET_PATH="${FORWARDER_BIN_PATH}"

if [[ ! -f "\${STAGED_PATH}" ]]; then
  exit 0
fi

tmp_target="\${TARGET_PATH}.tmp.\$\$"
install -m 0755 "\${STAGED_PATH}" "\${tmp_target}"
mv "\${tmp_target}" "\${TARGET_PATH}"
rm -f "\${STAGED_PATH}"
EOF
}

render_forwarder_systemd_unit() {
  cat <<EOF
[Unit]
Description=Remote Timing Forwarder (rt-forwarder)
Documentation=https://github.com/iwismer/rusty-timer
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
PermissionsStartOnly=true
ExecStartPre=${APPLY_STAGED_HELPER}
User=rt-forwarder
Group=rt-forwarder
ExecStart=/usr/local/bin/rt-forwarder
WorkingDirectory=/var/lib/rusty-timer
Environment=RUST_LOG=info
Environment=RT_FORWARDER_UPDATE_APPLY_VIA_RESTART=1
Restart=on-failure
RestartSec=5s
StartLimitInterval=60s
StartLimitBurst=5
StandardOutput=journal
StandardError=journal
SyslogIdentifier=rt-forwarder
AmbientCapabilities=CAP_NET_BIND_SERVICE
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/rusty-timer
ReadWritePaths=/etc/rusty-timer
TimeoutStopSec=30s

[Install]
WantedBy=multi-user.target
EOF
}

render_power_actions_sudoers() {
  cat <<EOF
# Allow rt-forwarder to issue host reboot/poweroff from the forwarder UI.
${SERVICE_USER} ALL=(root) NOPASSWD: /bin/systemctl --no-ask-password reboot
${SERVICE_USER} ALL=(root) NOPASSWD: /bin/systemctl --no-ask-password poweroff
${SERVICE_USER} ALL=(root) NOPASSWD: /usr/bin/systemctl --no-ask-password reboot
${SERVICE_USER} ALL=(root) NOPASSWD: /usr/bin/systemctl --no-ask-password poweroff
EOF
}

install_verify_policy() {
  local service_was_active="$1"
  local restart_answer="$2"

  if [[ "${service_was_active}" == "yes" && "${restart_answer}" =~ ^[Nn]$ ]]; then
    printf 'skip_verify\n'
  else
    printf 'run_verify\n'
  fi
}

require_root() {
  if [[ $EUID -ne 0 ]]; then
    echo "Error: this script must be run as root (sudo)." >&2
    exit 1
  fi
}

ensure_prerequisites() {
  echo "── Prerequisites ──"

  local missing=()
  local install_pkgs=()
  for cmd in curl jq tar sha256sum; do
      if ! command -v "${cmd}" >/dev/null 2>&1; then
        missing+=("${cmd}")
        case "${cmd}" in
          sha256sum) install_pkgs+=("coreutils") ;;
          *) install_pkgs+=("${cmd}") ;;
        esac
      fi
  done
  if [[ ${#missing[@]} -gt 0 ]]; then
      echo "Error: missing required commands: ${missing[*]}" >&2
      echo "Install with: sudo apt-get install -y ${install_pkgs[*]}" >&2
      exit 1
  fi

  # Create service user if it doesn't exist
  id -u "${SERVICE_USER}" &>/dev/null || \
    useradd -r -s /bin/false -m -d "${DATA_DIR}" "${SERVICE_USER}"

  # Create directories
  mkdir -p "${CONFIG_DIR}" "${DATA_DIR}"
  chown "${SERVICE_USER}:${SERVICE_USER}" "${CONFIG_DIR}"
  chmod 0750 "${CONFIG_DIR}"
  chown "${SERVICE_USER}:${SERVICE_USER}" "${DATA_DIR}"
}

# ── Functions ────────────────────────────────────────────────────────

download_binary() {
  echo "── Download binary ──"

  if [[ -f "${INSTALL_DIR}/rt-forwarder" ]]; then
    local should_redownload="0"
    if is_noninteractive_mode; then
      should_redownload="$(bool_env_is_true "${RT_SETUP_REDOWNLOAD:-0}")"
    else
      read -rp "rt-forwarder is already installed. Re-download? [y/N] " answer
      if [[ "${answer}" =~ ^[Yy]$ ]]; then
        should_redownload="1"
      fi
    fi

    if [[ "${should_redownload}" != "1" ]]; then
      echo "Skipping download."
      return
    fi
  fi

  echo "Fetching latest forwarder release from GitHub..."

  local releases_pages=()
  local page_json
  local page
  for page in {1..5}; do
    page_json=$(curl -fsSL "https://api.github.com/repos/${GITHUB_REPO}/releases?per_page=100&page=${page}")
    if [[ "${page_json}" == "[]" ]]; then
      break
    fi
    releases_pages+=("${page_json}")
  done

  # Find the latest stable release whose tag matches forwarder-v*
  local download_url
  download_url=$(select_latest_forwarder_asset_from_pages "${releases_pages[@]}")

  if [[ -z "${download_url}" || "${download_url}" == "null" ]]; then
    echo "Error: could not find a forwarder arm64 release asset." >&2
    exit 1
  fi

  echo "Downloading: ${download_url}"

  local tmp_dir
  local asset_name
  local checksum_url
  local expected_checksum
  local actual_checksum
  tmp_dir=$(mktemp -d)
  trap 'rm -rf "${tmp_dir}"' EXIT

  asset_name="${download_url##*/}"
  checksum_url="${download_url}.sha256"

  curl -fsSL "${download_url}" -o "${tmp_dir}/forwarder.tar.gz"
  curl -fsSL "${checksum_url}" -o "${tmp_dir}/forwarder.tar.gz.sha256"

  expected_checksum="$(checksum_for_asset_from_sha256sums "$(cat "${tmp_dir}/forwarder.tar.gz.sha256")" "${asset_name}")"
  if [[ -z "${expected_checksum}" ]]; then
    echo "Error: checksum file did not contain an entry for ${asset_name}" >&2
    exit 1
  fi

  actual_checksum="$(sha256sum "${tmp_dir}/forwarder.tar.gz" | awk '{print $1}')"
  if [[ "${expected_checksum}" != "${actual_checksum}" ]]; then
    echo "Error: checksum mismatch for ${asset_name}" >&2
    exit 1
  fi

  tar -xzf "${tmp_dir}/forwarder.tar.gz" -C "${tmp_dir}"
  mv "${tmp_dir}/forwarder" "${INSTALL_DIR}/rt-forwarder"
  chmod +x "${INSTALL_DIR}/rt-forwarder"

  rm -rf "${tmp_dir}"
  trap - EXIT

  echo "Installed rt-forwarder to ${INSTALL_DIR}/rt-forwarder"
}

configure() {
  echo ""
  echo "── Configure ──"

  if [[ -f "${CONFIG_DIR}/forwarder.toml" ]]; then
    local overwrite_config="0"
    if is_noninteractive_mode; then
      overwrite_config="$(bool_env_is_true "${RT_SETUP_OVERWRITE_CONFIG:-0}")"
    else
      read -rp "Config already exists. Overwrite? [y/N] " answer
      if [[ "${answer}" =~ ^[Yy]$ ]]; then
        overwrite_config="1"
      fi
    fi

    if [[ "${overwrite_config}" != "1" ]]; then
      echo "Skipping configuration."
      # Read existing bind address for verify() to use
      local existing_bind
      existing_bind=$(sed -n 's/^bind\s*=\s*"\([^"]*\)".*/\1/p' "${CONFIG_DIR}/forwarder.toml" 2>/dev/null || true)
      if [[ -n "${existing_bind}" ]]; then
        STATUS_BIND="${existing_bind}"
      fi
      return
    fi
  fi

  # Server base URL (required)
  local server_base_url="${RT_SETUP_SERVER_BASE_URL:-}"
  if is_noninteractive_mode; then
    if [[ -z "${server_base_url}" ]]; then
      echo "Error: RT_SETUP_SERVER_BASE_URL is required in non-interactive mode." >&2
      exit 1
    fi
    if [[ ! "${server_base_url}" =~ ^https?:// ]]; then
      echo "Error: RT_SETUP_SERVER_BASE_URL must start with http:// or https://." >&2
      exit 1
    fi
  else
    server_base_url=""
    while [[ -z "${server_base_url}" ]]; do
      read -rp "Server base URL: " server_base_url
      if [[ -z "${server_base_url}" ]]; then
        echo "Server base URL is required."
        continue
      fi
      if [[ ! "${server_base_url}" =~ ^https?:// ]]; then
        echo "Server base URL must start with http:// or https://"
        server_base_url=""
        continue
      fi
    done
  fi

  # Auth token (required, hidden input)
  local auth_token="${RT_SETUP_AUTH_TOKEN:-}"
  if is_noninteractive_mode; then
    if [[ -z "${auth_token}" ]]; then
      echo "Error: RT_SETUP_AUTH_TOKEN is required in non-interactive mode." >&2
      exit 1
    fi
  else
    auth_token=""
    while [[ -z "${auth_token}" ]]; do
      read -rsp "Auth token: " auth_token
      echo ""
      if [[ -z "${auth_token}" ]]; then
        echo "Auth token is required."
      fi
    done
  fi

  # Write token file
  mkdir -p "${CONFIG_DIR}"
  echo -n "${auth_token}" > "${CONFIG_DIR}/forwarder.token"
  chmod 600 "${CONFIG_DIR}/forwarder.token"
  chown "${SERVICE_USER}:${SERVICE_USER}" "${CONFIG_DIR}/forwarder.token"

  # Reader targets (at least one required)
  local readers=()
  if is_noninteractive_mode; then
    local env_reader
    while IFS= read -r env_reader; do
      if [[ -z "${env_reader}" ]]; then
        continue
      fi
      if ! is_valid_reader_target "${env_reader}"; then
        echo "Error: invalid reader target in RT_SETUP_READER_TARGETS: ${env_reader}" >&2
        exit 1
      fi
      readers+=("${env_reader}")
    done < <(reader_targets_from_env "${RT_SETUP_READER_TARGETS:-}")

    if [[ ${#readers[@]} -eq 0 ]]; then
      echo "Error: RT_SETUP_READER_TARGETS must include at least one target." >&2
      exit 1
    fi
  else
    echo "Enter reader targets. At least one is required."
    while true; do
      read -rp "Reader target (IP:PORT, or empty to finish): " target
      if [[ -z "${target}" ]]; then
        if [[ ${#readers[@]} -eq 0 ]]; then
          echo "At least one reader target is required."
          continue
        fi
        break
      fi
      if ! is_valid_reader_target "${target}"; then
        echo "Invalid format. Expected IP:PORT (e.g. 192.168.1.10:4000) or IP_RANGE:PORT (e.g. 192.168.1.150-160:10000)"
        continue
      fi
      readers+=("${target}")
    done
  fi

  # Status HTTP bind address
  if is_noninteractive_mode; then
    local env_status_bind="${RT_SETUP_STATUS_BIND:-}"
    if [[ -n "${env_status_bind}" ]]; then
      STATUS_BIND="${env_status_bind}"
    fi
  else
    local input_bind
    read -rp "Status HTTP bind address [${STATUS_BIND}]: " input_bind
    if [[ -n "${input_bind}" ]]; then
      STATUS_BIND="${input_bind}"
    fi
  fi

  # Forwarder display name defaults to host name.
  local forwarder_display_name="${RT_SETUP_DISPLAY_NAME:-}"
  if [[ -z "${forwarder_display_name}" ]]; then
    forwarder_display_name="$(default_forwarder_display_name)"
  fi
  local escaped_forwarder_display_name
  escaped_forwarder_display_name="$(toml_escape_string "${forwarder_display_name}")"
  local control_allow_power_actions
  control_allow_power_actions="$(allow_power_actions_toml_value)"

  # Generate config file
  cat > "${CONFIG_DIR}/forwarder.toml" <<EOF
schema_version = 1
display_name = "${escaped_forwarder_display_name}"

[server]
base_url = "${server_base_url}"

[auth]
token_file = "/etc/rusty-timer/forwarder.token"

[journal]
sqlite_path = "/var/lib/rusty-timer/forwarder.sqlite3"
prune_watermark_pct = 80

[status_http]
bind = "${STATUS_BIND}"

[control]
allow_power_actions = ${control_allow_power_actions}

[uplink]
batch_mode = "immediate"
batch_flush_ms = 100
batch_max_events = 50
EOF

  # Append reader targets
  for reader in "${readers[@]}"; do
    cat >> "${CONFIG_DIR}/forwarder.toml" <<EOF

[[readers]]
target = "${reader}"
read_type = "raw"
enabled = true
EOF
  done

  chown "${SERVICE_USER}:${SERVICE_USER}" "${CONFIG_DIR}/forwarder.toml"

  echo "Configuration written to ${CONFIG_DIR}/forwarder.toml"
}

install_service() {
  echo ""
  echo "── Install service ──"

  install -d -m 0755 "${HELPER_DIR}"
  render_apply_staged_script > "${APPLY_STAGED_HELPER}"
  chmod 0755 "${APPLY_STAGED_HELPER}"
  chown root:root "${APPLY_STAGED_HELPER}"

  # Write systemd unit file
  render_forwarder_systemd_unit > /etc/systemd/system/rt-forwarder.service

  # Allow power-action UI endpoints to run reboot/poweroff non-interactively.
  render_power_actions_sudoers > "${POWER_ACTIONS_SUDOERS_PATH}"
  chmod 0440 "${POWER_ACTIONS_SUDOERS_PATH}"
  chown root:root "${POWER_ACTIONS_SUDOERS_PATH}"
  if command -v visudo >/dev/null 2>&1; then
    visudo -cf "${POWER_ACTIONS_SUDOERS_PATH}" >/dev/null
  fi

  systemctl daemon-reload
  systemctl enable rt-forwarder

  local restart_answer=""

  if systemctl is-active --quiet rt-forwarder; then
    if is_noninteractive_mode; then
      if [[ "$(bool_env_is_true "${RT_SETUP_RESTART_IF_RUNNING:-1}")" == "1" ]]; then
        restart_answer="y"
      else
        restart_answer="n"
      fi
    else
      read -rp "Service is already running. Restart now? [Y/n] " answer
      restart_answer="${answer}"
    fi
    VERIFY_POLICY="$(install_verify_policy "yes" "${restart_answer}")"
    if [[ "${VERIFY_POLICY}" == "skip_verify" ]]; then
      echo "Service not restarted. Run 'sudo systemctl restart rt-forwarder' when ready."
      return
    fi
    systemctl restart rt-forwarder
  else
    VERIFY_POLICY="$(install_verify_policy "no" "${restart_answer}")"
    systemctl start rt-forwarder
  fi

  echo "Service installed and started."
}

verify() {
  echo ""
  echo "── Verify ──"

  sleep 3

  local probe_url
  probe_url="$(status_probe_url_from_bind "${STATUS_BIND}")"
  local failed=0

  if systemctl is-active --quiet rt-forwarder; then
    echo "rt-forwarder is running."
  else
    echo "rt-forwarder is NOT running."
    failed=1
  fi

  if curl -fsS "${probe_url}"; then
    echo ""
    echo "Health check passed."
  else
    echo "Health check failed at ${probe_url}."
    failed=1
  fi

  if [[ ${failed} -ne 0 ]]; then
    echo "Check logs with: journalctl -u rt-forwarder -n 50"
    return 1
  fi
}

main() {
  echo "=== rt-forwarder Setup ==="
  echo ""

  if is_noninteractive_mode \
    && [[ -n "${RT_SETUP_DONE_MARKER:-}" ]] \
    && [[ -f "${RT_SETUP_DONE_MARKER}" ]]; then
    echo "Setup marker already present at ${RT_SETUP_DONE_MARKER}; skipping."
    return 0
  fi

  require_root
  ensure_prerequisites

  download_binary
  configure
  install_service

  if [[ "${VERIFY_POLICY}" == "run_verify" ]]; then
    verify
  else
    local probe_url
    probe_url="$(status_probe_url_from_bind "${STATUS_BIND}")"
    echo "Verification skipped because restart was deferred."
    echo "After restarting, verify with:"
    echo "  sudo systemctl restart rt-forwarder"
    echo "  curl -fsS ${probe_url}"
  fi

  write_done_marker_if_requested

  echo ""
  echo "Setup complete."
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main
fi
