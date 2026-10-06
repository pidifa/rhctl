#!/bin/bash

# ****************************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ****************************************************************************************************
# Creates the pgvector extension in a specified PostgreSQL database using a superuser.
#
# If the extension already exists, skip — safe to re-run.
#
# Usage:
#   ./enable-pgvector.sh \
#     --db-name pidifa \
#     --port 2334
#
# Required Parameters:
#   --db-name <name>
#       Target PostgreSQL database in which to create the pgvector extension.
#         Example name values: `pidifa`, `pidifa_dev`
#
# Optional Parameters:
#   --port <port>
#       PostgreSQL server port (default: `5432`).
#         Example port values: `5432`, `2334`
#
#   --superuser <name>
#       PostgreSQL superuser used to create the extension (default: `postgres`).
#         Example user values: `postgres`, `admin`
#
# Override Parameters:
#   RHCTL_PG_DB_NAME=<name>
#       Same as `--db-name`.
#
#   RHCTL_PG_PORT=<port>
#       Same as `--port`.
#
#   RHCTL_PG_SUPERUSER=<name>
#       Same as `--superuser`.
#
# Since : 1.0.4
# Date  : Tue Oct 6, 2026
# ****************************************************************************************************

set -euo pipefail

# ========================================================================= Defaults & Overrides

DB_NAME="${RHCTL_PG_DB_NAME:-}"
PG_PORT="${RHCTL_PG_PORT:-5432}"
PG_SUPERUSER="${RHCTL_PG_SUPERUSER:-postgres}"

# ========================================================================= Parameter parsing

while [[ $# -gt 0 ]]; do
    case "$1" in
        --db-name)
            if [[ $# -lt 2 ]]; then
                echo "Error: --db-name requires a value." >&2
                exit 1
            fi

            DB_NAME="$2"
            shift 2
            ;;

        --port)
            if [[ $# -lt 2 ]]; then
                echo "Error: --port requires a value." >&2
                exit 1
            fi

            PG_PORT="$2"
            shift 2
            ;;

        --superuser)
            if [[ $# -lt 2 ]]; then
                echo "Error: --superuser requires a value." >&2
                exit 1
            fi

            PG_SUPERUSER="$2"
            shift 2
            ;;

        -h|--help)
            echo "Usage: $0 --db-name <name> [--port <port>] [--superuser <name>]"
            exit 0
            ;;

        *)
            echo "Error: Unknown argument '$1'." >&2
            exit 1
            ;;
    esac
done

# ========================================================================= Parameter validation

if [[ -z "$DB_NAME" ]]; then
    echo "Error: --db-name is required." >&2
    echo "Example: --db-name pidifa" >&2
    exit 1
fi

if [[ ! "$PG_PORT" =~ ^[0-9]+$ ]]; then
    echo "Error: Invalid PostgreSQL port: $PG_PORT" >&2
    echo "Example: --port 5432" >&2
    exit 1
fi

if [[ -z "$PG_SUPERUSER" ]]; then
    echo "Error: PostgreSQL superuser name cannot be empty." >&2
    exit 1
fi

# ========================================================================= Command checks

if ! command -v psql >/dev/null 2>&1; then
    echo "Error: 'psql' command was not found." >&2
    echo "Please install PostgreSQL client tools first." >&2
    exit 1
fi

if [[ $EUID -ne 0 ]]; then
    if ! command -v sudo >/dev/null 2>&1; then
        echo "Error: This script must be run as root or have 'sudo' installed." >&2
        exit 1
    fi

    if ! sudo -n -u "$PG_SUPERUSER" true >/dev/null 2>&1; then
        echo "Error: Cannot execute commands as PostgreSQL superuser '$PG_SUPERUSER'." >&2
        echo "Run this script as root or configure sudo access for '$PG_SUPERUSER'." >&2
        exit 1
    fi

    SUDO="sudo -n -u $PG_SUPERUSER"
else
    SUDO="runuser -u $PG_SUPERUSER --"
fi

# ========================================================================= PostgreSQL connection

echo "Checking PostgreSQL connection..."

if ! $SUDO psql \
    --port="$PG_PORT" \
    --dbname="$DB_NAME" \
    --command="SELECT 1;" \
    >/dev/null; then

    echo "Error: Failed to connect to database '$DB_NAME' on port $PG_PORT." >&2
    exit 1
fi

# ========================================================================= Superuser verification

IS_SUPERUSER="$(
    $SUDO psql \
        --port="$PG_PORT" \
        --dbname="$DB_NAME" \
        --tuples-only \
        --no-align \
        --command="SELECT rolsuper FROM pg_roles WHERE rolname = current_user;"
)"

IS_SUPERUSER="$(echo "$IS_SUPERUSER" | tr -d '[:space:]')"

if [[ "$IS_SUPERUSER" != "t" ]]; then
    echo "Error: PostgreSQL user '$PG_SUPERUSER' is not a superuser." >&2
    exit 1
fi

echo "Using PostgreSQL superuser: $PG_SUPERUSER"

# ========================================================================= Extension check

EXTENSION_EXISTS="$(
    $SUDO psql \
        --port="$PG_PORT" \
        --dbname="$DB_NAME" \
        --tuples-only \
        --no-align \
        --command="SELECT 1 FROM pg_extension WHERE extname = 'vector';"
)"

EXTENSION_EXISTS="$(echo "$EXTENSION_EXISTS" | tr -d '[:space:]')"

if [[ "$EXTENSION_EXISTS" == "1" ]]; then
    EXTENSION_VERSION="$(
        $SUDO psql \
            --port="$PG_PORT" \
            --dbname="$DB_NAME" \
            --tuples-only \
            --no-align \
            --command="SELECT extversion FROM pg_extension WHERE extname = 'vector';"
    )"

    EXTENSION_VERSION="$(echo "$EXTENSION_VERSION" | tr -d '[:space:]')"

    echo "pgvector extension is already installed in database '$DB_NAME'."
    echo "Version: $EXTENSION_VERSION"
    echo "Skip — safe to re-run."
    exit 0
fi

# ========================================================================= Create extension

echo "Creating pgvector extension in database '$DB_NAME'..."

$SUDO psql \
    --port="$PG_PORT" \
    --dbname="$DB_NAME" \
    --command="CREATE EXTENSION IF NOT EXISTS vector;"

# ========================================================================= Verification

EXTENSION_VERSION="$(
    $SUDO psql \
        --port="$PG_PORT" \
        --dbname="$DB_NAME" \
        --tuples-only \
        --no-align \
        --command="SELECT extversion FROM pg_extension WHERE extname = 'vector';"
)"

EXTENSION_VERSION="$(echo "$EXTENSION_VERSION" | tr -d '[:space:]')"

if [[ -z "$EXTENSION_VERSION" ]]; then
    echo "Error: pgvector extension creation failed." >&2
    exit 1
fi

# ========================================================================= Final output

echo
echo "Successfully created pgvector extension."
echo "Database : $DB_NAME"
echo "Port     : $PG_PORT"
echo "Version  : $EXTENSION_VERSION"
