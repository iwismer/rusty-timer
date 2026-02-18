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
CONFIG_DIR="/etc/rusty-timer"
DATA_DIR="/var/lib/rusty-timer"
SERVICE_USER="rt-forwarder"
STATUS_BIND="0.0.0.0:8080"

# ── Helpers ──────────────────────────────────────────────────────────

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
    | select((.name // "") | test("forwarder-.*-linux-arm64\\.tar\\.gz$"))
    | .browser_download_url
    | select(type == "string" and length > 0)
  ' | head -n 1
}

status_probe_url_from_bind() {
  local bind="$1"
  local port="8080"

  if [[ "${bind}" =~ ^\[[0-9A-Fa-f:]+\]:(.+)$ ]]; then
    port="${BASH_REMATCH[1]}"
  elif [[ "${bind}" == *:* ]]; then
    port="${bind##*:}"
  fi

  printf 'http://localhost:%s/healthz' "${port}"
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
  for cmd in curl jq tar; do
      command -v "${cmd}" >/dev/null 2>&1 || missing+=("${cmd}")
  done
  if [[ ${#missing[@]} -gt 0 ]]; then
      echo "Error: missing required commands: ${missing[*]}" >&2
      echo "Install with: sudo apt-get install -y ${missing[*]}" >&2
      exit 1
  fi

  # Create service user if it doesn't exist
  id -u "${SERVICE_USER}" &>/dev/null || \
    useradd -r -s /bin/false -m -d "${DATA_DIR}" "${SERVICE_USER}"

  # Create directories
  mkdir -p "${CONFIG_DIR}" "${DATA_DIR}"
  chown "${SERVICE_USER}:${SERVICE_USER}" "${DATA_DIR}"
}

# ── Functions ────────────────────────────────────────────────────────

download_binary() {
  echo "── Download binary ──"

  if [[ -f "${INSTALL_DIR}/rt-forwarder" ]]; then
    read -rp "rt-forwarder is already installed. Re-download? [y/N] " answer
    if [[ ! "${answer}" =~ ^[Yy]$ ]]; then
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
  tmp_dir=$(mktemp -d)
  trap 'rm -rf "${tmp_dir}"' EXIT

  curl -fsSL "${download_url}" -o "${tmp_dir}/forwarder.tar.gz"
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
    read -rp "Config already exists. Overwrite? [y/N] " answer
    if [[ ! "${answer}" =~ ^[Yy]$ ]]; then
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
  local server_base_url=""
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

  # Auth token (required, hidden input)
  local auth_token=""
  while [[ -z "${auth_token}" ]]; do
    read -rsp "Auth token: " auth_token
    echo ""
    if [[ -z "${auth_token}" ]]; then
      echo "Auth token is required."
    fi
  done

  # Write token file
  mkdir -p "${CONFIG_DIR}"
  echo -n "${auth_token}" > "${CONFIG_DIR}/forwarder.token"
  chmod 600 "${CONFIG_DIR}/forwarder.token"
  chown "${SERVICE_USER}:${SERVICE_USER}" "${CONFIG_DIR}/forwarder.token"

  # Reader targets (at least one required)
  local readers=()
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
    if [[ ! "${target}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+:[0-9]+$ && ! "${target}" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+-[0-9]+:[0-9]+$ ]]; then
      echo "Invalid format. Expected IP:PORT (e.g. 192.168.1.10:4000) or IP_RANGE:PORT (e.g. 192.168.1.150-160:10000)"
      continue
    fi
    readers+=("${target}")
  done

  # Status HTTP bind address
  local input_bind
  read -rp "Status HTTP bind address [${STATUS_BIND}]: " input_bind
  if [[ -n "${input_bind}" ]]; then
    STATUS_BIND="${input_bind}"
  fi

  # Generate config file
  cat > "${CONFIG_DIR}/forwarder.toml" <<EOF
schema_version = 1

[server]
base_url = "${server_base_url}"

[auth]
token_file = "/etc/rusty-timer/forwarder.token"

[journal]
sqlite_path = "/var/lib/rusty-timer/forwarder.sqlite3"
prune_watermark_pct = 80

[status_http]
bind = "${STATUS_BIND}"

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

  # Write systemd unit file
  cat > /etc/systemd/system/rt-forwarder.service <<'EOF'
[Unit]
Description=Remote Timing Forwarder (rt-forwarder)
Documentation=https://github.com/iwismer/rusty-timer
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=rt-forwarder
Group=rt-forwarder
ExecStart=/usr/local/bin/rt-forwarder
WorkingDirectory=/var/lib/rusty-timer
Environment=RUST_LOG=info
Restart=on-failure
RestartSec=5s
StartLimitInterval=60s
StartLimitBurst=5
StandardOutput=journal
StandardError=journal
SyslogIdentifier=rt-forwarder
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
ReadWritePaths=/var/lib/rusty-timer
ReadOnlyPaths=/etc/rusty-timer
TimeoutStopSec=30s

[Install]
WantedBy=multi-user.target
EOF

  systemctl daemon-reload
  systemctl enable rt-forwarder

  if systemctl is-active --quiet rt-forwarder; then
    read -rp "Service is already running. Restart now? [Y/n] " answer
    if [[ "${answer}" =~ ^[Nn]$ ]]; then
      echo "Service not restarted. Run 'sudo systemctl restart rt-forwarder' when ready."
      return
    fi
    systemctl restart rt-forwarder
  else
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

  require_root
  ensure_prerequisites

  download_binary
  configure
  install_service
  verify

  echo ""
  echo "Setup complete."
}

if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main
fi
