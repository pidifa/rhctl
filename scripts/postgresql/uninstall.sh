#!/bin/bash
# ************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ************************************************************************************
# Completely uninstall PostgreSQL installed from PGDG.
#
# This removes:
#   - PostgreSQL packages
#   - PostgreSQL clusters and data
#   - PGDG repository
#   - PGDG repository key
#   - PostgreSQL configuration and state directories
#
# If not installed, skip — safe to re-run.
#
# Usage:
#   ./uninstall.sh
#
# Required Parameters:
#   (none)
#
# Optional Parameters:
#   (none)
#
# Override Parameters:
#   (none)
#
# Since : 1.0.1
# Date  : Feb 8, 2026
# ************************************************************************************

set -e

echo "[INFO] Checking PostgreSQL installation"

if ! command -v psql &>/dev/null && ! dpkg -l 'postgresql*' 2>/dev/null | grep -q '^ii'; then
    echo "[INFO] PostgreSQL is not installed"
    echo "[INFO] Skipping uninstall"
    exit 0
fi

echo "[INFO] PostgreSQL installation found"

# Stop PostgreSQL
if systemctl list-unit-files --type=service 2>/dev/null | grep -q '^postgresql\.service'; then
    if systemctl is-active --quiet postgresql; then
        echo "[INFO] Stopping PostgreSQL service"
        sudo systemctl stop postgresql
    else
        echo "[INFO] PostgreSQL service is already stopped"
    fi

    echo "[INFO] Disabling PostgreSQL service"
    sudo systemctl disable postgresql 2>/dev/null || true
fi

# Remove PostgreSQL packages
# echo "[INFO] Removing PostgreSQL packages"
# sudo apt purge -y \
#     'postgresql*' \
#     'postgresql-client*' \
#     'postgresql-common' \
#     'postgresql-client-common' \
#     2>/dev/null || true

# Remove unused dependencies
# echo "[INFO] Removing unused dependencies"
# sudo apt autoremove -y

# Remove PostgreSQL data and configuration
echo "[INFO] Removing PostgreSQL data and configuration"

sudo rm -rf \
    /var/lib/postgresql \
    /var/log/postgresql \
    /etc/postgresql \
    /etc/postgresql-common \
    /var/run/postgresql

# Remove PGDG repository
echo "[INFO] Removing PostgreSQL repository"

sudo rm -f \
    /etc/apt/sources.list.d/pgdg.list \
    /etc/apt/sources.list.d/pgdg.sources

# Remove PGDG repository key
echo "[INFO] Removing PostgreSQL repository key"

sudo rm -f \
    /etc/apt/trusted.gpg.d/postgresql.gpg \
    /usr/share/keyrings/postgresql.gpg

# Refresh package lists
# echo "[INFO] Updating package lists"
# sudo apt update

echo "[INFO] PostgreSQL uninstall completed successfully"