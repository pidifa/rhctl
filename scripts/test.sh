#!/usr/bin/env bash
# ************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ************************************************************************************
# Test rhctl against README examples for execute, transfer, patch, and run commands.
#
# Each test runs a README example against the configured remote host and reports
# PASS/FAIL. Tests requiring a remote server are skipped if the host is unreachable.
#
# Usage:
#   ./scripts/test.sh [--host HOST] [--user USER] [--password PASS]
#
# Override Parameters:
#   RHCTL_HOST=<host>         Remote host (default: 192.168.75.128)
#   RHCTL_USER=<user>         Remote user (default: test99)
#   RHCTL_PASSWORD=<pass>     Remote password (default: testpwd)
#   RHCTL_SSH_PORT=<port>     SSH port (default: 22)
#   RHCTL_BIN=<path>          Path to rhctl binary (default: rhctl from PATH)
#
# Since : 1.0.5
# Date  : Nov 8, 2026
# ************************************************************************************

set -uo pipefail

# ========================================================================= Parameter
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

RHCTL_HOST="${RHCTL_HOST:-192.168.75.128}"
RHCTL_USER="${RHCTL_USER:-test99}"
RHCTL_PASSWORD="${RHCTL_PASSWORD:-testpwd}"
RHCTL_SSH_PORT="${RHCTL_SSH_PORT:-22}"
RHCTL_BIN="${RHCTL_BIN:-rhctl}"

# Parse CLI overrides
while [[ $# -gt 0 ]]; do
  case "$1" in
    --host)      RHCTL_HOST="$2"; shift 2 ;;
    --user)      RHCTL_USER="$2"; shift 2 ;;
    --password)  RHCTL_PASSWORD="$2"; shift 2 ;;
    --ssh-port)  RHCTL_SSH_PORT="$2"; shift 2 ;;
    --rhctl-bin) RHCTL_BIN="$2"; shift 2 ;;
    *) echo "Unknown option: $1" >&2; exit 1 ;;
  esac
done

ASSETS_ROOT="${PROJECT_ROOT}/assets"

PASS_COUNT=0
FAIL_COUNT=0
SKIP_COUNT=0
FAILED_TESTS=()

# ========================================================================= Methods
log_pass() { echo "  [PASS] $1"; PASS_COUNT=$((PASS_COUNT + 1)); }
log_fail() { echo "  [FAIL] $1"; FAIL_COUNT=$((FAIL_COUNT + 1)); FAILED_TESTS+=("$1"); }
log_skip() { echo "  [SKIP] $1"; SKIP_COUNT=$((SKIP_COUNT + 1)); }

section_header() {
  echo ""
  echo "============================================================"
  echo " $1"
  echo "============================================================"
}

check_rhctl_installed() {
  if ! command -v "${RHCTL_BIN}" >/dev/null 2>&1; then
    echo "ERROR: rhctl binary '${RHCTL_BIN}' not found in PATH." >&2
    echo "  Build and install first: ./scripts/build.sh" >&2
    exit 1
  fi
  echo "Using rhctl: $(command -v "${RHCTL_BIN}")"
  "${RHCTL_BIN}" --version >/dev/null 2>&1 && "${RHCTL_BIN}" --version || true
}

check_host_reachable() {
  if ! ping -c 1 -W 3 "${RHCTL_HOST}" >/dev/null 2>&1; then
    return 1
  fi
  return 0
}

# ========================================================================= Test: execute
test_execute() {
  section_header "rhctl execute"
  if ! check_host_reachable; then
    log_skip "execute (host ${RHCTL_HOST} unreachable)"
    return
  fi

  echo "Test 1: execute with local scripts (async, --use-sudo)"
  "${RHCTL_BIN}" execute \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --script "${ASSETS_ROOT}/example-bash1.sh" \
    --script "${ASSETS_ROOT}/example-bash2.sh" \
    --mode async \
    --use-sudo \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "execute with scripts (async, --use-sudo)"
  else
    log_fail "execute with scripts (async, --use-sudo)"
  fi

  echo "Test 2: execute with direct remote command"
  "${RHCTL_BIN}" execute \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --cmd "hostname" \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "execute with remote command"
  else
    log_fail "execute with remote command"
  fi

  echo "Test 3: execute with script parameters"
  "${RHCTL_BIN}" execute \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --script "${ASSETS_ROOT}/example-bash1.sh" \
    --mode sync \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "execute with script parameters (sync)"
  else
    log_fail "execute with script parameters (sync)"
  fi
}

# ========================================================================= Test: transfer
test_transfer() {
  section_header "rhctl transfer"
  if ! check_host_reachable; then
    log_skip "transfer (host ${RHCTL_HOST} unreachable)"
    return
  fi

  echo "Test 1: transfer upload (file + directory)"
  "${RHCTL_BIN}" transfer \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --upload "${ASSETS_ROOT}/example1.txt=~/examples" \
    --upload "${ASSETS_ROOT}/exampledir=~/examples/targetdir" \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "transfer upload (file + directory)"
  else
    log_fail "transfer upload (file + directory)"
  fi

  echo "Test 2: transfer download"
  "${RHCTL_BIN}" transfer \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --download "~/examples/example1.txt=/tmp/rhctl-test-downloads" \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "transfer download"
  else
    log_fail "transfer download"
  fi

  echo "Test 3: transfer upload mapping file"
  "${RHCTL_BIN}" transfer \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --upload-mapping "${PROJECT_ROOT}/config/path-mapping.properties" \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "transfer upload mapping file"
  else
    log_fail "transfer upload mapping file"
  fi

  echo "Test 4: transfer patch-env"
  "${RHCTL_BIN}" transfer \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --patch-env "${ASSETS_ROOT}/example1.txt=/tmp/rhctl-test-env" \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "transfer patch-env"
  else
    log_fail "transfer patch-env"
  fi

  echo "Test 5: transfer upload with --use-rsync"
  "${RHCTL_BIN}" transfer \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --upload "${ASSETS_ROOT}/example1.txt=~/examples/rsync-test" \
    --use-rsync \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "transfer upload with --use-rsync"
  else
    log_fail "transfer upload with --use-rsync"
  fi

  echo "Test 6: transfer download with --use-rsync"
  "${RHCTL_BIN}" transfer \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --download "~/examples/example1.txt=/tmp/rhctl-test-downloads-rsync" \
    --use-rsync \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "transfer download with --use-rsync"
  else
    log_fail "transfer download with --use-rsync"
  fi
}

# ========================================================================= Test: patch
test_patch() {
  section_header "rhctl patch"
  if ! check_host_reachable; then
    log_skip "patch (host ${RHCTL_HOST} unreachable)"
    return
  fi

  echo "Test 1: patch upload, backup, and overwrite"
  "${RHCTL_BIN}" patch \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --local-path "${ASSETS_ROOT}/example-patch.txt" \
    --remote-upload "/tmp/rhctl-test-patch-upload.txt" \
    --remote-path "~/examples/example-patch-remote.txt" \
    --remote-backup "/tmp/rhctl-test-patch-remote.bak" \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "patch (upload, backup, overwrite)"
  else
    log_fail "patch (upload, backup, overwrite)"
  fi

  echo "Test 2: patch recover from backup"
  "${RHCTL_BIN}" patch \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --local-path "${ASSETS_ROOT}/example-patch.txt" \
    --remote-upload "/tmp/rhctl-test-patch-upload.txt" \
    --remote-path "~/examples/example-patch-remote.txt" \
    --remote-backup "/tmp/rhctl-test-patch-remote.bak" \
    --recover \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "patch (recover from backup)"
  else
    log_fail "patch (recover from backup)"
  fi
}

# ========================================================================= Test: run
test_run() {
  section_header "rhctl run"
  if ! check_host_reachable; then
    log_skip "run (host ${RHCTL_HOST} unreachable)"
    return
  fi

  echo "Test 1: run with config.yml (dev-deploy)"
  "${RHCTL_BIN}" run \
    --host "${RHCTL_HOST}" \
    --user "${RHCTL_USER}" \
    --ssh-port "${RHCTL_SSH_PORT}" \
    --password "${RHCTL_PASSWORD}" \
    --config "${PROJECT_ROOT}/config.yml" \
    --config-name dev-deploy \
    --debug
  if [[ $? -eq 0 ]]; then
    log_pass "run with config.yml (dev-deploy)"
  else
    log_fail "run with config.yml (dev-deploy)"
  fi
}

# ========================================================================= Main
main() {
  echo "rhctl README Example Test Suite"
  echo "  Host: ${RHCTL_HOST}:${RHCTL_SSH_PORT}"
  echo "  User: ${RHCTL_USER}"
  echo "  Binary: ${RHCTL_BIN}"
  echo "  Assets: ${ASSETS_ROOT}"

  check_rhctl_installed

  test_execute
  test_transfer
  test_patch
  test_run

  # ========================================================================= Summary
  echo ""
  echo "============================================================"
  echo " Test Summary"
  echo "============================================================"
  echo "  PASS: ${PASS_COUNT}"
  echo "  FAIL: ${FAIL_COUNT}"
  echo "  SKIP: ${SKIP_COUNT}"

  if [[ ${FAIL_COUNT} -gt 0 ]]; then
    echo ""
    echo "Failed tests:"
    for t in "${FAILED_TESTS[@]}"; do
      echo "  - ${t}"
    done
    exit 1
  fi

  echo ""
  echo "All tests passed!"
  exit 0
}

main "$@"
