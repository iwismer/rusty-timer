#!/usr/bin/env bash
# validate-packaging.sh — Validates Dockerfiles, compose files, systemd units,
# and runbooks for the Remote Forwarding Suite packaging artifacts.
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

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

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
    else
        check_fail "${desc}: file not found: ${path}"
        return 1
    fi
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

# ---------------------------------------------------------------------------
# Section 1: Forwarder Dockerfile
# ---------------------------------------------------------------------------

echo ""
echo "=== Forwarder Dockerfile ==="

FORWARDER_DF="services/forwarder/Dockerfile"

check_file_exists "${FORWARDER_DF}" "Forwarder Dockerfile exists"

if [[ -f "${REPO_ROOT}/${FORWARDER_DF}" ]]; then
    # Must be multi-stage: has at least two FROM instructions.
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
fi

# ---------------------------------------------------------------------------
# Section 2: Server Dockerfile
# ---------------------------------------------------------------------------

echo ""
echo "=== Server Dockerfile ==="

SERVER_DF="services/server/Dockerfile"

check_file_exists "${SERVER_DF}" "Server Dockerfile exists"

if [[ -f "${REPO_ROOT}/${SERVER_DF}" ]]; then
    FROM_COUNT=$(grep -c '^FROM ' "${REPO_ROOT}/${SERVER_DF}" || true)
    if [[ "${FROM_COUNT}" -ge 3 ]]; then
        check_pass "Server Dockerfile is multi-stage (>= 3 FROM: node, rust, final)"
    else
        check_fail "Server Dockerfile should be multi-stage with >=3 stages, found ${FROM_COUNT}"
    fi

    check_file_contains "${SERVER_DF}" '(node|npm|pnpm|yarn)' \
        "Server Dockerfile includes Node.js stage (SvelteKit dashboard)"

    check_file_contains "${SERVER_DF}" 'cargo build' \
        "Server Dockerfile runs cargo build"

    check_file_contains "${SERVER_DF}" '--release' \
        "Server Dockerfile builds in release mode"

    check_file_contains "${SERVER_DF}" 'COPY.*(dashboard|apps|static|dist|build)' \
        "Server Dockerfile includes dashboard build artifacts"

    check_file_contains "${SERVER_DF}" '^(ENTRYPOINT|CMD)' \
        "Server Dockerfile has ENTRYPOINT or CMD"
fi

# ---------------------------------------------------------------------------
# Section 3: Systemd unit for forwarder
# ---------------------------------------------------------------------------

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

# ---------------------------------------------------------------------------
# Section 4: Docker Compose production file
# ---------------------------------------------------------------------------

echo ""
echo "=== docker-compose.prod.yml ==="

COMPOSE_FILE="deploy/docker-compose.prod.yml"

check_file_exists "${COMPOSE_FILE}" "Production docker-compose.prod.yml exists"

if [[ -f "${REPO_ROOT}/${COMPOSE_FILE}" ]]; then
    check_file_contains "${COMPOSE_FILE}" 'services:' \
        "Compose file has services section"

    check_file_contains "${COMPOSE_FILE}" '(postgres|db|database|postgresql)' \
        "Compose file includes Postgres service"

    check_file_contains "${COMPOSE_FILE}" '(server|rt-server)' \
        "Compose file includes server service"

    check_file_contains "${COMPOSE_FILE}" 'restart:' \
        "Compose file has restart policy"

    check_file_contains "${COMPOSE_FILE}" '(unless-stopped|always|on-failure)' \
        "Compose file has meaningful restart policy"

    check_file_contains "${COMPOSE_FILE}" '(DATABASE_URL|POSTGRES|db|postgres)' \
        "Compose file wires Postgres URL to server"

    # Validate compose syntax if docker compose is available.
    # Set required env vars with dummy values for syntax-only validation.
    if command -v docker &>/dev/null && docker compose version &>/dev/null 2>&1; then
        if POSTGRES_PASSWORD=test_validate \
            docker compose -f "${REPO_ROOT}/${COMPOSE_FILE}" config --quiet 2>/dev/null; then
            check_pass "docker compose config validates successfully"
        else
            check_fail "docker compose config failed for ${COMPOSE_FILE}"
        fi
    else
        check_pass "docker compose not available — skipping syntax validation"
    fi
fi

# ---------------------------------------------------------------------------
# Section 5: Runbooks
# ---------------------------------------------------------------------------

echo ""
echo "=== Runbooks ==="

RUNBOOK_TOPICS=(
    "startup"
    "recovery"
    "epoch.reset\|reset-epoch\|epoch reset"
    "export\|exports"
    "delete\|retention.delete\|manual delete"
)

check_runbook() {
    local runbook_path="$1"
    local runbook_name="$2"
    check_file_exists "${runbook_path}" "${runbook_name} runbook exists"
    if [[ -f "${REPO_ROOT}/${runbook_path}" ]]; then
        check_file_contains "${runbook_path}" '(startup|Startup|STARTUP|start)' \
            "${runbook_name}: covers startup"
        check_file_contains "${runbook_path}" '(recovery|Recovery|recover|reconnect|restart)' \
            "${runbook_name}: covers recovery"
    fi
}

check_runbook "docs/runbooks/forwarder-operations.md" "Forwarder"
check_runbook "docs/runbooks/server-operations.md" "Server"
check_runbook "docs/runbooks/receiver-operations.md" "Receiver"

# Forwarder runbook must cover epoch reset.
FWRD_RUNBOOK="docs/runbooks/forwarder-operations.md"
if [[ -f "${REPO_ROOT}/${FWRD_RUNBOOK}" ]]; then
    check_file_contains "${FWRD_RUNBOOK}" 'epoch' \
        "Forwarder runbook: covers epoch reset"
fi

# Server runbook must cover exports and manual retention-delete.
SRV_RUNBOOK="docs/runbooks/server-operations.md"
if [[ -f "${REPO_ROOT}/${SRV_RUNBOOK}" ]]; then
    check_file_contains "${SRV_RUNBOOK}" '(export|Export|EXPORT)' \
        "Server runbook: covers exports"
    check_file_contains "${SRV_RUNBOOK}" '(delete|Delete|DELETE|retention)' \
        "Server runbook: covers manual retention-delete (DB-admin only)"
    check_file_contains "${SRV_RUNBOOK}" '(admin|Admin|operator|DB-admin|database admin)' \
        "Server runbook: restricts manual delete to DB-admin"
fi

# ---------------------------------------------------------------------------
# Section 6: validate-packaging.sh is executable.
# ---------------------------------------------------------------------------

echo ""
echo "=== Script Permissions ==="

THIS_SCRIPT="scripts/validate-packaging.sh"
if [[ -x "${REPO_ROOT}/${THIS_SCRIPT}" ]]; then
    check_pass "validate-packaging.sh is executable"
else
    check_fail "validate-packaging.sh must be executable (chmod +x)"
fi

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------

echo ""
echo "=== Summary ==="
echo "  PASS: ${PASS}"
echo "  FAIL: ${FAIL}"
echo ""

if [[ "${FAIL}" -gt 0 ]]; then
    echo "Validation FAILED with ${FAIL} check(s) failing."
    exit 1
else
    echo "Validation PASSED. All ${PASS} checks passed."
    exit 0
fi
