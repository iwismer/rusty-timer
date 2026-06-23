#!/usr/bin/env bash
# validate-packaging.sh — Validates release packaging artifacts for the
# P2P Remote Forwarding Suite.
#
# Exits 0 if all checks pass, non-zero if any check fails.
#
# Usage: ./scripts/validate-packaging.sh [--verbose]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VERBOSE=false

if [[ "${1:-}" == "--verbose" ]]; then
    VERBOSE=true
fi

PASS=0
FAIL=0

check_pass() {
    local desc="$1"
    PASS=$((PASS + 1))
    if [[ "${VERBOSE}" == "true" ]]; then
        echo "  [PASS] ${desc}"
    fi
}

check_fail() {
    local desc="$1"
    FAIL=$((FAIL + 1))
    echo "  [FAIL] ${desc}"
}

check_file_exists() {
    local path="$1"
    local desc="$2"
    if [[ -f "${REPO_ROOT}/${path}" ]]; then
        check_pass "${desc}"
        return 0
    fi
    check_fail "${desc}: file not found: ${path}"
    return 1
}

check_file_absent() {
    local path="$1"
    local desc="$2"
    if [[ ! -e "${REPO_ROOT}/${path}" ]]; then
        check_pass "${desc}"
        return 0
    fi
    check_fail "${desc}: stale path still exists: ${path}"
    return 1
}

check_file_contains() {
    local path="$1"
    local pattern="$2"
    local desc="$3"
    if grep -qE -- "${pattern}" "${REPO_ROOT}/${path}" 2>/dev/null; then
        check_pass "${desc}"
    else
        check_fail "${desc}: pattern '${pattern}' not found in ${path}"
    fi
}

check_file_not_contains() {
    local path="$1"
    local pattern="$2"
    local desc="$3"
    if grep -qE -- "${pattern}" "${REPO_ROOT}/${path}" 2>/dev/null; then
        check_fail "${desc}: stale pattern '${pattern}' found in ${path}"
    else
        check_pass "${desc}"
    fi
}

echo ""
echo "=== Cutover removals (legacy WebSocket/Postgres data plane) ==="

# The central "server" component name is reused by the current P2P registry
# service; only the legacy WebSocket/Postgres data-plane artifacts are guarded.
check_file_absent "crates/rt-protocol" "Legacy WebSocket protocol crate removed"
check_file_absent "contracts/ws" "Legacy WebSocket contract removed"

echo ""
echo "=== Forwarder Dockerfile ==="

FORWARDER_DF="services/forwarder/Dockerfile"
check_file_exists "${FORWARDER_DF}" "Forwarder Dockerfile exists"

if [[ -f "${REPO_ROOT}/${FORWARDER_DF}" ]]; then
    FROM_COUNT=$(grep -c '^FROM ' "${REPO_ROOT}/${FORWARDER_DF}" || true)
    if [[ "${FROM_COUNT}" -ge 2 ]]; then
        check_pass "Forwarder Dockerfile is multi-stage (>= 2 FROM)"
    else
        check_fail "Forwarder Dockerfile must be multi-stage (found ${FROM_COUNT} FROM)"
    fi

    check_file_contains "${FORWARDER_DF}" 'cargo build' \
        "Forwarder Dockerfile runs cargo build"
    check_file_contains "${FORWARDER_DF}" '--release' \
        "Forwarder Dockerfile builds in release mode"
    check_file_contains "${FORWARDER_DF}" 'COPY.*(forwarder|services)' \
        "Forwarder Dockerfile copies forwarder source"
    check_file_contains "${FORWARDER_DF}" '^(ENTRYPOINT|CMD)' \
        "Forwarder Dockerfile has ENTRYPOINT or CMD"
    check_file_contains "${FORWARDER_DF}" '(HEALTHCHECK|healthz|readyz)' \
        "Forwarder Dockerfile references health endpoint"
    check_file_not_contains "${FORWARDER_DF}" 'rt-protocol|tokio-tungstenite|postgres' \
        "Forwarder Dockerfile has no legacy data-plane references"
fi

echo ""
echo "=== Server Docker deployment ==="

SERVER_DF="services/server/Dockerfile"
SERVER_COMPOSE="deploy/server/docker-compose.yml"
SERVER_CADDY="deploy/server/Caddyfile.example"
SERVER_ENV="deploy/server/.env.example"
SERVER_DEPLOY_README="deploy/server/README.md"
DOCKERIGNORE=".dockerignore"

check_file_exists "${SERVER_DF}" "Server Dockerfile exists"
check_file_exists "${SERVER_COMPOSE}" "Server Docker Compose sample exists"
check_file_exists "${SERVER_CADDY}" "Server Caddy config sample exists"
check_file_exists "${SERVER_ENV}" "Server deploy env example exists"
check_file_exists "${SERVER_DEPLOY_README}" "Server Docker deploy README exists"
check_file_exists "${DOCKERIGNORE}" "Docker ignore file exists"

if [[ -f "${REPO_ROOT}/${SERVER_DF}" ]]; then
    FROM_COUNT=$(grep -c '^FROM ' "${REPO_ROOT}/${SERVER_DF}" || true)
    if [[ "${FROM_COUNT}" -ge 3 ]]; then
        check_pass "Server Dockerfile is multi-stage (>= 3 FROM)"
    else
        check_fail "Server Dockerfile must build UI, Rust binary, and runtime stages (found ${FROM_COUNT} FROM)"
    fi

    check_file_contains "${SERVER_DF}" 'npm run build --workspace apps/server-ui' \
        "Server Dockerfile builds server UI"
    check_file_contains "${SERVER_DF}" 'apps/forwarder-ui/package.json' \
        "Server Dockerfile copies all npm workspace manifests"
    check_file_contains "${SERVER_DF}" 'apps/receiver-ui/package.json' \
        "Server Dockerfile copies all npm workspace manifests"
    check_file_contains "${SERVER_DF}" 'cargo build.*--release.*--package server.*--bin server.*--features embed-ui' \
        "Server Dockerfile builds server with embedded UI"
    check_file_contains "${SERVER_DF}" 'SERVER_DB_PATH|/var/lib/rusty-timer-server' \
        "Server Dockerfile uses SQLite data path"
    check_file_contains "${SERVER_DF}" '(HEALTHCHECK|healthz)' \
        "Server Dockerfile references health endpoint"
    check_file_not_contains "${SERVER_DF}" 'Postgres|postgres|DATABASE_URL|DASHBOARD_DIR|rt-protocol|tokio-tungstenite' \
        "Server Dockerfile has no legacy data-plane references"
fi

if [[ -f "${REPO_ROOT}/${SERVER_COMPOSE}" ]]; then
    check_file_contains "${SERVER_COMPOSE}" 'caddy:' \
        "Server Compose includes Caddy"
    check_file_contains "${SERVER_COMPOSE}" 'SERVER_DB_PATH' \
        "Server Compose configures SQLite path"
    check_file_contains "${SERVER_COMPOSE}" 'SERVER_TRUSTED_PROXY' \
        "Server Compose enables trusted proxy for admin routes"
    check_file_contains "${SERVER_COMPOSE}" 'server_data:' \
        "Server Compose persists server SQLite data"
    check_file_not_contains "${SERVER_COMPOSE}" 'postgres|DATABASE_URL' \
        "Server Compose has no Postgres dependency"
fi

if [[ -f "${REPO_ROOT}/${SERVER_CADDY}" ]]; then
    check_file_contains "${SERVER_CADDY}" 'request_header -Remote-User' \
        "Caddy sample strips spoofable Remote-User"
    check_file_contains "${SERVER_CADDY}" 'method GET' \
        "Caddy sample pins public/device read routes to GET"
    check_file_contains "${SERVER_CADDY}" 'path /healthz /status' \
        "Caddy sample leaves health/status public"
    check_file_contains "${SERVER_CADDY}" 'method POST' \
        "Caddy sample pins device write routes to POST"
    check_file_contains "${SERVER_CADDY}" 'path /register /forwarder/catalog /announcer/rows /announcer/takeover' \
        "Caddy sample leaves M2M POST bearer routes unproxied by Authelia"
    check_file_contains "${SERVER_CADDY}" 'path /forwarders /allowlist/receivers' \
        "Caddy sample leaves M2M GET bearer routes unproxied by Authelia"
    check_file_contains "${SERVER_CADDY}" 'forward_auth' \
        "Caddy sample protects admin/UI routes with forward_auth"
    check_file_contains "${SERVER_CADDY}" 'copy_headers Remote-User' \
        "Caddy sample forwards authenticated admin identity"
fi

if [[ -f "${REPO_ROOT}/${DOCKERIGNORE}" ]]; then
    check_file_contains "${DOCKERIGNORE}" '^target$' \
        "Docker ignore excludes Rust build output"
    check_file_contains "${DOCKERIGNORE}" 'apps/\*/node_modules' \
        "Docker ignore excludes frontend dependencies"
    check_file_contains "${DOCKERIGNORE}" 'apps/\*/build' \
        "Docker ignore excludes stale frontend build output"
fi

echo ""
echo "=== Systemd Unit: rt-forwarder.service ==="

SYSTEMD_UNIT="deploy/systemd/rt-forwarder.service"
check_file_exists "${SYSTEMD_UNIT}" "Forwarder systemd unit exists"

if [[ -f "${REPO_ROOT}/${SYSTEMD_UNIT}" ]]; then
    check_file_contains "${SYSTEMD_UNIT}" '^\[Unit\]' \
        "Systemd unit has [Unit] section"
    check_file_contains "${SYSTEMD_UNIT}" '^\[Service\]' \
        "Systemd unit has [Service] section"
    check_file_contains "${SYSTEMD_UNIT}" '^\[Install\]' \
        "Systemd unit has [Install] section"
    check_file_contains "${SYSTEMD_UNIT}" 'ExecStart' \
        "Systemd unit has ExecStart"
    check_file_contains "${SYSTEMD_UNIT}" 'Restart=' \
        "Systemd unit has Restart policy"
    check_file_contains "${SYSTEMD_UNIT}" 'WantedBy=(multi-user|network).target' \
        "Systemd unit targets multi-user or network"
fi

echo ""
echo "=== Runbooks ==="

check_runbook() {
    local runbook_path="$1"
    local runbook_name="$2"
    check_file_exists "${runbook_path}" "${runbook_name} runbook exists"
    if [[ -f "${REPO_ROOT}/${runbook_path}" ]]; then
        check_file_contains "${runbook_path}" '(startup|Startup|STARTUP|start)' \
            "${runbook_name}: covers startup"
        check_file_contains "${runbook_path}" '(recovery|Recovery|recover|reconnect|restart)' \
            "${runbook_name}: covers recovery"
        check_file_not_contains "${runbook_path}" '(Postgres|WebSocket|rt-protocol)' \
            "${runbook_name}: has no legacy data-plane references"
    fi
}

check_runbook "docs/runbooks/forwarder-operations.md" "Forwarder"
check_runbook "docs/runbooks/receiver-operations.md" "Receiver"
check_runbook "docs/runbooks/server-operations.md" "Server"

FWRD_RUNBOOK="docs/runbooks/forwarder-operations.md"
if [[ -f "${REPO_ROOT}/${FWRD_RUNBOOK}" ]]; then
    check_file_contains "${FWRD_RUNBOOK}" 'epoch' \
        "Forwarder runbook: covers epoch operations"
fi

SERVER_RUNBOOK="docs/runbooks/server-operations.md"
if [[ -f "${REPO_ROOT}/${SERVER_RUNBOOK}" ]]; then
    check_file_contains "${SERVER_RUNBOOK}" '(enrollment token|enrollment voucher|minted.*device token)' \
        "Server runbook: covers enrollment-token provisioning"
    check_file_not_contains "${SERVER_RUNBOOK}" 'SERVER_PROVISIONING_TOKEN|shared provisioning token' \
        "Server runbook: has no stale shared provisioning-token instructions"
    check_file_contains "${SERVER_RUNBOOK}" '(allow-list|allowlist|allow list)' \
        "Server runbook: covers allow-list distribution"
    check_file_contains "${SERVER_RUNBOOK}" '(announcer|generation|lease)' \
        "Server runbook: covers announcer push"
    check_file_contains "${SERVER_RUNBOOK}" '(Authelia|M2M|Bearer|public read)' \
        "Server runbook: covers auth posture"
fi

echo ""
echo "=== Release workflow ==="

RELEASE_WF=".github/workflows/release.yml"
check_file_exists "${RELEASE_WF}" "Release workflow exists"
if [[ -f "${REPO_ROOT}/${RELEASE_WF}" ]]; then
    check_file_contains "${RELEASE_WF}" 'server-v\*' \
        "Release workflow publishes server tags"
    check_file_contains "${RELEASE_WF}" 'aarch64-unknown-linux-gnu' \
        "Release workflow includes Linux arm64 server artifact target"
    check_file_contains "${RELEASE_WF}" 'services/server/Dockerfile' \
        "Release workflow publishes server Docker image"
    check_file_contains "${RELEASE_WF}" 'linux/amd64' \
        "Release workflow builds server Docker image for amd64"
    check_file_contains "${RELEASE_WF}" 'needs: \[resolve-matrix, build\]' \
        "Release workflow creates GitHub release independent of Docker Hub push"
    check_file_contains "${RELEASE_WF}" "env.SERVICE == 'forwarder' \|\| env.SERVICE == 'server'" \
        "Release workflow builds/checks server UI"
    check_file_contains "${RELEASE_WF}" 'embed-ui' \
        "Release workflow embeds server UI"
    check_file_not_contains "${RELEASE_WF}" '(armv7)' \
        "Release workflow has no armv7 packaging"
fi

echo ""
echo "=== Release helper ==="

RELEASE_HELPER="scripts/release.py"
check_file_exists "${RELEASE_HELPER}" "Release helper exists"
if [[ -f "${REPO_ROOT}/${RELEASE_HELPER}" ]]; then
    check_file_contains "${RELEASE_HELPER}" 'EMBED_UI_SERVICES = \("forwarder", "server"\)' \
        "Release helper treats server as embedded-UI service"
    check_file_contains "${RELEASE_HELPER}" '"server": "apps/server-ui"' \
        "Release helper maps server UI workspace"
    check_file_contains "${RELEASE_HELPER}" 'npm", "run", "build", "--workspace", ui_workspace' \
        "Release helper builds embedded UI assets before cargo"
fi

echo ""
echo "=== Script Permissions ==="

THIS_SCRIPT="scripts/validate-packaging.sh"
if [[ -x "${REPO_ROOT}/${THIS_SCRIPT}" ]]; then
    check_pass "validate-packaging.sh is executable"
else
    check_fail "validate-packaging.sh must be executable (chmod +x)"
fi

echo ""
echo "=== Summary ==="
echo "  PASS: ${PASS}"
echo "  FAIL: ${FAIL}"
echo ""

if [[ "${FAIL}" -gt 0 ]]; then
    echo "Validation FAILED with ${FAIL} check(s) failing."
    exit 1
fi

echo "Validation PASSED. All ${PASS} checks passed."
exit 0
