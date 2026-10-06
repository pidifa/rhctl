#!/bin/bash

# ****************************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ****************************************************************************************************
# Automates installing pgvector for PostgreSQL via APT package manager.
# Automatically detects the installed PostgreSQL version and installs postgresql-<ver>-pgvector.
#
# If installed, skip — safe to re-run.
#
# Usage:
#   ./install-pgvector.sh \
#     --pg-version 16
#
# Required Parameters:
#   (none)
#
# Optional Parameters:
#   --pg-version <version>
#       Target PostgreSQL major version. Auto-detected if omitted.
#       Example version values: `14`, `15`, `16`, `17`
#   --force
#       Reinstall or upgrade even if pgvector is already present.
#
# Override Parameters:
#   PGVERSION=<version>
#       Same as `--pg-version`.
#
# Since : 1.0.0
# Date  : Tue Oct 6, 2026
# ****************************************************************************************************

set -euo pipefail

# ========================================================================= Defaults & Overrides

PGVERSION="${PGVERSION:-}"
FORCE=false

# ========================================================================= Parameter parsing

while [[ $# -gt 0 ]]; do
    case "$1" in
        --pg-version)
            if [[ $# -lt 2 ]]; then
                echo "Error: --pg-version requires a value." >&2
                exit 1
            fi

            PGVERSION="$2"
            shift 2
            ;;

        --version)
            # Package version is handled by APT repository, retain flag for parameter compatibility
            if [[ $# -lt 2 ]]; then
                echo "Error: --version requires a value." >&2
                exit 1
            fi
            shift 2
            ;;

        --force)
            FORCE=true
            shift
            ;;

        -h|--help)
            echo "Usage: $0 [--pg-version <ver>] [--force]"
            exit 0
            ;;

        *)
            echo "Error: Unknown argument '$1'." >&2
            exit 1
            ;;
    esac
done

# ========================================================================= OS & APT Check

if ! command -v apt-get >/dev/null 2>&1; then
    echo "Error: This script requires 'apt-get' package manager (Debian/Ubuntu)." >&2
    exit 1
fi

# ========================================================================= Parameter validation

if [[ -n "$PGVERSION" && ! "$PGVERSION" =~ ^[0-9]+$ ]]; then
    echo "Error: Invalid PostgreSQL major version: $PGVERSION" >&2
    echo "Example: --pg-version 16" >&2
    exit 1
fi

# ========================================================================= PostgreSQL discovery

if [[ -z "$PGVERSION" ]]; then
    # Automatically detect PostgreSQL major version
    if command -v pg_config >/dev/null 2>&1; then
        RAW_VER="$(pg_config --version | awk '{print $2}')"
        PGVERSION="${RAW_VER%%.*}"
    elif command -v psql >/dev/null 2>&1; then
        RAW_VER="$(psql --version | awk '{print $3}')"
        PGVERSION="${RAW_VER%%.*}"
    else
        # Fallback search for common installed directories
        for path in /usr/lib/postgresql/*/bin/pg_config; do
            if [[ -x "$path" ]]; then
                RAW_VER="$("$path" --version | awk '{print $2}')"
                PGVERSION="${RAW_VER%%.*}"
                break
            fi
        done
    fi
fi

if [[ -z "$PGVERSION" ]]; then
    echo "Error: Could not automatically detect PostgreSQL major version." >&2
    echo "Please specify it manually using '--pg-version <version>'." >&2
    exit 1
fi

echo "Target PostgreSQL major version: $PGVERSION"

PACKAGE_NAME="postgresql-${PGVERSION}-pgvector"

# ========================================================================= Re-run / Skip check

CONTROL_FILE="/usr/share/postgresql/${PGVERSION}/extension/vector.control"

if [[ -f "$CONTROL_FILE" && "$FORCE" == "false" ]]; then
    echo "pgvector extension is already installed ($CONTROL_FILE)."
    echo "Skip — safe to re-run."
    exit 0
fi

if [[ -f "$CONTROL_FILE" && "$FORCE" == "true" ]]; then
    echo "pgvector extension is already installed."
    echo "Reinstalling because --force was specified."
fi

# ========================================================================= Package Installation

echo "Updating APT package index and installing $PACKAGE_NAME..."

SUDO=""
if [[ $EUID -ne 0 ]]; then
    if command -v sudo >/dev/null 2>&1; then
        SUDO="sudo"
    else
        echo "Error: This script must be run as root or have 'sudo' installed." >&2
        exit 1
    fi
fi

$SUDO apt-get update -qq
$SUDO apt-get install -y "$PACKAGE_NAME"

# ========================================================================= Verification

if [[ -f "$CONTROL_FILE" ]] || dpkg -s "$PACKAGE_NAME" >/dev/null 2>&1; then
    echo
    echo "Successfully installed $PACKAGE_NAME."
    echo
    echo "Run the following SQL in each database that needs pgvector:"
    echo
    echo "  CREATE EXTENSION IF NOT EXISTS vector;"
    echo
else
    echo "Error: Installation finished, but package '$PACKAGE_NAME' validation failed." >&2
    exit 1
fi