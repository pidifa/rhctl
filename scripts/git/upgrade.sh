#!/bin/bash
# ************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ************************************************************************************
# Upgrade the rhctl version across user-specified files.
#
# Replaces the old semantic version with the new semantic version in each
# specified file (both `v1.0.9` and `1.0.9` forms), then commits and pushes
# the changes to the main branch.
#
# If exists, overwrite — safe to re-run.
#
# Usage:
#   ./scripts/git/upgrade.sh \
#     --repo /path/to/repository \
#     --old-version 1.0.9 \
#     --new-version 1.0.10 \
#     --file main/Cargo.toml \
#     --file docs/release-notes.md
#
# Required Parameters:
#   --repo <directory>
#       Git repository root directory.
#         Example directory values: `/home/user/project`, `/mnt/c/Users/user/project`
#
#   --old-version <version>
#       The current semantic version (e.g. `1.0.9`).
#         Example version values: `1.0.9`, `1.1.0`, `2.0.0`
#
#   --new-version <version>
#       The new semantic version to set (e.g. `1.0.10`).
#         Example version values: `1.0.10`, `1.1.0`, `2.0.0`
#
#   --file <relative-path>
#       File (relative to the repository root) in which to replace the old
#       version with the new version. May be repeated.
#         Example path values: `main/Cargo.toml`, `docs/release-notes.md`
#
# Optional Parameters:
#   -h, --help
#       Show this help (default: off).
#         Example values: (none)
#
# Since : 1.0.6
# Date  : Mon Nov 9, 2026
# ************************************************************************************

set -euo pipefail

# ========================================================================= Parameter

REPO_DIR=""
OLD_VERSION=""
NEW_VERSION=""
FILES=()

# ========================================================================= Methods

usage() {
  sed -n '1,75p' "$0"
  exit 1
}

error() {
  echo "Error: $*" >&2
  exit 1
}

require_tool() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: '$1' not found on PATH. $2" >&2
    exit 1
  fi
}

is_semver() {
  # Accepts versions like 1.0.6, 1.1.0, 2.0.0 (major.minor.patch)
  [[ "$1" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]
}

# ========================================================================= Argument parsing

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)
      [[ $# -ge 2 ]] || error "Missing value for --repo."
      REPO_DIR="$2"
      shift 2
      ;;
    --old-version)
      [[ $# -ge 2 ]] || error "Missing value for --old-version."
      OLD_VERSION="$2"
      shift 2
      ;;
    --new-version)
      [[ $# -ge 2 ]] || error "Missing value for --new-version."
      NEW_VERSION="$2"
      shift 2
      ;;
    --file)
      [[ $# -ge 2 ]] || error "Missing value for --file."
      FILES+=("$2")
      shift 2
      ;;
    -h | --help)
      usage
      ;;
    *)
      error "Unknown parameter: $1"
      ;;
  esac
done

[[ -n "$REPO_DIR" ]] || error "--repo is required."

[[ -n "$OLD_VERSION" ]] || error "--old-version is required."
[[ -n "$NEW_VERSION" ]] || error "--new-version is required."
[[ ${#FILES[@]} -gt 0 ]] || error "At least one --file is required."

is_semver "$OLD_VERSION" \
  || error "Invalid old version '$OLD_VERSION'. Expected major.minor.patch, e.g. 1.0.9."

is_semver "$NEW_VERSION" \
  || error "Invalid new version '$NEW_VERSION'. Expected major.minor.patch, e.g. 1.0.10."

# ========================================================================= Environment checks

require_tool git "Install Git: https://git-scm.com/"

# ========================================================================= Repository discovery

[[ -d "$REPO_DIR" ]] \
    || error "Repository directory does not exist: ${REPO_DIR}"

REPO_DIR="$(cd "$REPO_DIR" && pwd)"

git -C "$REPO_DIR" rev-parse --is-inside-work-tree >/dev/null 2>&1 \
    || error "Not a Git repository: ${REPO_DIR}"

SCRIPT_EXEC_DIR="$(pwd)"
trap 'cd "$SCRIPT_EXEC_DIR"' EXIT

REPO_ROOT="$(git -C "$REPO_DIR" rev-parse --show-toplevel)"

cd "$REPO_ROOT"

# ========================================================================= Validate files exist

for rel in "${FILES[@]}"; do
  [[ -f "$REPO_ROOT/$rel" ]] \
    || error "File not found: $REPO_ROOT/$rel"
done

# ========================================================================= Replace version in each file

OLD_VERSION_ESCAPED="${OLD_VERSION//./\.}"

for rel in "${FILES[@]}"; do
  FILE="$REPO_ROOT/$rel"
  echo "==> Updating $rel: $OLD_VERSION -> $NEW_VERSION"
  sed -i -E "s/v${OLD_VERSION_ESCAPED}/v${NEW_VERSION}/g; s/${OLD_VERSION_ESCAPED}/${NEW_VERSION}/g" "$FILE"
done

# ========================================================================= Commit

git add "${FILES[@]}"
git commit -m "chore: upgrade version to $NEW_VERSION"
git push origin main

# ========================================================================= Final output

echo "Upgraded version: $OLD_VERSION -> $NEW_VERSION"
for rel in "${FILES[@]}"; do
  echo "  Updated: $rel"
done
echo "Committed and pushed to main branch."
