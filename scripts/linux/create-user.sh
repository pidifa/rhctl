#!/bin/bash

# ************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ************************************************************************************
# Create a system user with an optional home directory and assign ownership
# of one or more existing paths.
#
# If the user exists, skip user creation — safe to re-run.
# If the home directory does not exist, create it.
# If an owned path exists, assign ownership — safe to re-run.
#
# Usage:
#   ./create-user.sh \
#     --user test \
#     --own /opt/test \
#     --own /testdir
#
#   ./create-user.sh \
#     --user test \
#     --no-home \
#     --own /opt/test
#
#   ./create-user.sh \
#     --user test \
#     --home /opt/test \
#     --own /opt/test
#
# Required Parameters:
#   --user <name>
#       System user to create.
#         Example name values: `test`, `test-gateway`
#
# Optional Parameters:
#   --home <path>
#       Home directory to create for the user (default: `/home/<user>`).
#         Example path values: `/home/test`, `/opt/test`
#
#   --no-home
#       Do not create a home directory.
#         The user's home is set to `/nonexistent`.
#
#   --own <path>
#       Directory or path to assign to the user's ownership.
#         May be specified multiple times.
#         Only existing paths are processed.
#         Example path values: `/opt/test`, `/app`
#
#   --shell <path>
#       Login shell for the user (default: `/usr/sbin/nologin`).
#         Example shell values: `/usr/sbin/nologin`, `/bin/bash`
#
# Override Parameters:
#   RHCTL_USER=<name>
#       Same as `--user`.
#
#   RHCTL_USER_HOME=<path>
#       Same as `--home`.
#
#   RHCTL_USER_SHELL=<path>
#       Same as `--shell`.
#
# Since : 1.0.4
# Date  : Oct 6, 2026
# ************************************************************************************

set -euo pipefail

# ========================================================================= Parameter

USER_NAME="${RHCTL_USER:-}"
USER_HOME=""
USER_SHELL="${RHCTL_USER_SHELL:-/usr/sbin/nologin}"
OWN_PATHS=()
NO_HOME=false
HOME_SPECIFIED=false

usage() {
    sed -n '1,95p' "$0"
    exit 0
}

die() {
    echo "Error: $*" >&2
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --user)
            [[ $# -ge 2 ]] || die "--user requires a value"
            USER_NAME="$2"
            shift 2
            ;;

        --home)
            [[ $# -ge 2 ]] || die "--home requires a value"
            USER_HOME="$2"
            HOME_SPECIFIED=true
            shift 2
            ;;

        --no-home)
            NO_HOME=true
            shift
            ;;

        --own)
            [[ $# -ge 2 ]] || die "--own requires a value"
            OWN_PATHS+=("$2")
            shift 2
            ;;

        --shell)
            [[ $# -ge 2 ]] || die "--shell requires a value"
            USER_SHELL="$2"
            shift 2
            ;;

        -h|--help)
            usage
            ;;

        *)
            die "Unknown parameter: $1"
            ;;
    esac
done

# ========================================================================= Validation

[[ -n "$USER_NAME" ]] || die "--user is required"

if [[ "$EUID" -ne 0 ]]; then
    die "This script must be run as root."
fi

if [[ ! "$USER_NAME" =~ ^[a-z_][a-z0-9_-]*[$]?$ ]]; then
    die "Invalid user name: $USER_NAME"
fi

if [[ ! "$USER_SHELL" = /* ]]; then
    die "Shell path must be absolute: $USER_SHELL"
fi

if [[ "$NO_HOME" == true && "$HOME_SPECIFIED" == true ]]; then
    die "--home and --no-home cannot be used together."
fi

if [[ "$NO_HOME" == true ]]; then
    USER_HOME="/nonexistent"
elif [[ "$HOME_SPECIFIED" == false ]]; then
    USER_HOME="/home/$USER_NAME"
fi

# ========================================================================= User creation

if id "$USER_NAME" &>/dev/null; then
    echo "User already exists: $USER_NAME"
else
    if [[ "$NO_HOME" == true ]]; then
        useradd \
            --system \
            --user-group \
            --home-dir "$USER_HOME" \
            --no-create-home \
            --shell "$USER_SHELL" \
            "$USER_NAME"

        echo "Created system user: $USER_NAME"
        echo "Home directory: none"
    else
        useradd \
            --system \
            --user-group \
            --home-dir "$USER_HOME" \
            --create-home \
            --shell "$USER_SHELL" \
            "$USER_NAME"

        echo "Created system user: $USER_NAME"
        echo "Created home directory: $USER_HOME"
    fi
fi

# ========================================================================= Home directory

if [[ "$NO_HOME" == false ]]; then
    if [[ ! -d "$USER_HOME" ]]; then
        mkdir -p "$USER_HOME"
        chown "$USER_NAME:$USER_NAME" "$USER_HOME"
        echo "Created home directory: $USER_HOME"
    fi
fi

# ========================================================================= Ownership

for path in "${OWN_PATHS[@]}"; do
    if [[ ! -e "$path" ]]; then
        echo "Skip ownership — path does not exist: $path"
        continue
    fi

    chown -R "$USER_NAME:$USER_NAME" "$path"
    echo "Assigned ownership: $USER_NAME:$USER_NAME -> $path"
done

# ========================================================================= Final output

echo
echo "User:  $USER_NAME"
echo "Group: $USER_NAME"
echo "Home:  $USER_HOME"
echo "Shell: $USER_SHELL"

if [[ ${#OWN_PATHS[@]} -gt 0 ]]; then
    echo "Owned paths:"
    for path in "${OWN_PATHS[@]}"; do
        echo "  $path"
    done
else
    echo "Owned paths: none"
fi