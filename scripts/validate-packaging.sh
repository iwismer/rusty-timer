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
echo "=== Cutover removals ==="

check_file_absent "services/server" "Legacy central service removed"
check_file_absent "deploy/server" "Legacy central deployment removed"
check_file_absent "apps/server-ui" "Legacy server dashboard removed"
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
    check_file_not_contains "${FORWARDER_DF}" 'services/server|rt-protocol|tokio-tungstenite|postgres' \
        "Forwarder Dockerfile has no legacy server data-plane references"
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
        check_file_not_contains "${runbook_path}" '(rt-server|Postgres|WebSocket|deploy/server|services/server|rt-protocol)' \
            "${runbook_name}: has no legacy server data-plane references"
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

THIN_RUNBOOK="docs/runbooks/server-operations.md"
if [[ -f "${REPO_ROOT}/${THIN_RUNBOOK}" ]]; then
    check_file_contains "${THIN_RUNBOOK}" '(SERVER_PROVISIONING_TOKEN|provisioning token)' \
        "Server runbook: covers provisioning token"
    check_file_contains "${THIN_RUNBOOK}" '(allow-list|allowlist|allow list)' \
        "Server runbook: covers allow-list distribution"
    check_file_contains "${THIN_RUNBOOK}" '(announcer|generation|lease)' \
        "Server runbook: covers announcer push"
    check_file_contains "${THIN_RUNBOOK}" '(Authelia|M2M|Bearer|public read)' \
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
        "Release workflow includes Linux arm64 target"
    check_file_not_contains "${RELEASE_WF}" '(server-v\*|SERVER_DOCKER_IMAGE|rt-server|services/server|apps/server-ui|armv7)' \
        "Release workflow has no legacy server or armv7 packaging"
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
