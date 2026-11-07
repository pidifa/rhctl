#!/usr/bin/env bash
# ************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ************************************************************************************
# Build rhctl in release mode and permanently install the binary into /usr/local/bin.
#
# If /usr/local/bin/rhctl already exists, it is overwritten — safe to re-run.
#
# Usage:
#   ./scripts/build.sh
#
# Override Parameters:
#   RHCTL_RELEASE_DIR=<path>
#       Directory containing the release binary (default: main/target/release).
#   RHCTL_INSTALL_DIR=<path>
#       Installation directory (default: /usr/local/bin).
#   RHCTL_BIN_NAME=<name>
#       Binary name (default: rhctl).
#
# Since : 1.0.5
# Date  : Nov 8, 2026
# ************************************************************************************

set -euo pipefail

# ========================================================================= Parameter
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

RELEASE_DIR="${RHCTL_RELEASE_DIR:-${PROJECT_ROOT}/main/target/release}"
INSTALL_DIR="${RHCTL_INSTALL_DIR:-/usr/local/bin}"
BIN_NAME="${RHCTL_BIN_NAME:-rhctl}"
BIN_PATH="${RELEASE_DIR}/${BIN_NAME}"
INSTALL_PATH="${INSTALL_DIR}/${BIN_NAME}"

# ========================================================================= Validate
echo "Building rhctl in release mode..."
cd "${PROJECT_ROOT}/main"
cargo build --release

if [[ ! -f "${BIN_PATH}" ]]; then
  echo "Release binary not found at ${BIN_PATH} after build" >&2
  exit 1
fi

if [[ ! -x "${BIN_PATH}" ]]; then
  echo "Binary ${BIN_PATH} is not executable" >&2
  exit 1
fi

# ========================================================================= Install
echo "Installing ${BIN_NAME} to ${INSTALL_PATH}..."
sudo mkdir -p "${INSTALL_DIR}"
sudo cp "${BIN_PATH}" "${INSTALL_PATH}"
sudo chmod +x "${INSTALL_PATH}"

# ========================================================================= Verify
INSTALLED_BIN="$(command -v "${BIN_NAME}" 2>/dev/null || true)"
if [[ -n "${INSTALLED_BIN}" ]]; then
  INSTALLED_VERSION="$("${INSTALLED_BIN}" --version 2>/dev/null || echo "unknown")"
  echo "Successfully installed ${BIN_NAME} (${INSTALLED_VERSION}) at ${INSTALLED_BIN}"
else
  echo "Warning: ${BIN_NAME} is not in PATH. You may need to add ${INSTALL_DIR} to your PATH."
  echo "  export PATH=\"${INSTALL_DIR}:$PATH\""
  echo "Installed binary: ${INSTALL_PATH}"
fi
