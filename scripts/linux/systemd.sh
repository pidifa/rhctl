#!/bin/bash

# ****************************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ****************************************************************************************************
# Manage rhctl-created systemd services.
#
# Service files are identified by the "${RHCTL_SERVICE_PREFIX}" prefix.
#
# Safe to re-run.
#
# Usage:
#   ./systemd.sh list
#   ./systemd.sh start --service rhctl-test-gateway-1.0.0
#   ./systemd.sh stop --service rhctl-test-gateway-1.0.0
#   ./systemd.sh restart --service rhctl-test-gateway-1.0.0
#   ./systemd.sh remove --service rhctl-test-gateway-1.0.0
#   ./systemd.sh daemon-reload
#
# Subcommands:
#   list
#       List all rhctl-created systemd services.
#
#   start
#       Start an rhctl-created systemd service.
#
#   stop
#       Stop an rhctl-created systemd service.
#
#   restart
#       Restart an rhctl-created systemd service.
#
#   remove
#       Stop, disable, and permanently remove an rhctl-created service.
#
#   daemon-reload
#       Reload systemd unit files after service files are created, modified, or removed.
#
# Parameters (list):
#   (none)
#
# Parameters (start):
#   --service <name>
#       Service name to start (required).
#         Example name values: `rhctl-test-gateway-1.0.0`, `rhctl-k3s-1.0.0`
#
# Parameters (stop):
#   --service <name>
#       Service name to stop (required).
#
# Parameters (restart):
#   --service <name>
#       Service name to restart (required).
#
# Parameters (remove):
#   --service <name>
#       Service name to remove (required).
#
# Parameters (daemon-reload):
#   (none)
#
# Override Parameters:
#   (none)
#
# Since : 1.0.4
# Date  : Oct 5, 2026
# ****************************************************************************************************

set -euo pipefail

readonly RHCTL_SERVICE_PREFIX="rhctl-"
readonly SYSTEMD_SERVICE_DIR="/etc/systemd/system"

readonly SCRIPT_NAME="$(basename "$0")"


# ========================================================================= Usage

usage() {
    cat <<EOF
Usage:
  ${SCRIPT_NAME} list
  ${SCRIPT_NAME} start --service <name>
  ${SCRIPT_NAME} stop --service <name>
  ${SCRIPT_NAME} restart --service <name>
  ${SCRIPT_NAME} remove --service <name>
  ${SCRIPT_NAME} daemon-reload

Subcommands:
  list
      List all rhctl-created systemd services.

  start
      Start an rhctl-created systemd service.

  stop
      Stop an rhctl-created systemd service.

  restart
      Restart an rhctl-created systemd service.

  remove
      Stop, disable, and permanently remove an rhctl-created service.

  daemon-reload
      Reload systemd unit files.

Parameters (start):
  --service <name>
      Service name to start (required).

Parameters (stop):
  --service <name>
      Service name to stop (required).

Parameters (restart):
  --service <name>
      Service name to restart (required).

Parameters (remove):
  --service <name>
      Service name to remove (required).
EOF
}


# ========================================================================= systemd discovery

list_services() {
    systemctl list-unit-files \
        --type=service \
        --no-legend \
        --no-pager |
        awk -v prefix="${RHCTL_SERVICE_PREFIX}" '
            $1 ~ "^" prefix && $1 ~ /\.service$/ {
                print $1
            }
        '
}


# ========================================================================= Service validation

normalize_service_name() {
    local service="$1"

    if [[ "${service}" != *.service ]]; then
        service="${service}.service"
    fi

    printf '%s\n' "${service}"
}


validate_service_name() {
    local service="$1"

    if [[ "${service}" != "${RHCTL_SERVICE_PREFIX}"*.service ]]; then
        echo "Error: service '${service}' is not an rhctl service." >&2
        echo "       Service names must start with '${RHCTL_SERVICE_PREFIX}'." >&2
        return 1
    fi
}


# ========================================================================= Service operation

service_operation() {
    local operation="$1"
    local service="$2"

    service="$(normalize_service_name "${service}")"
    validate_service_name "${service}"

    systemctl "${operation}" "${service}"
}


# ========================================================================= Service removal

remove_service() {
    local service
    service="$(normalize_service_name "$1")"

    validate_service_name "${service}"

    local service_file="${SYSTEMD_SERVICE_DIR}/${service}"

    if systemctl is-active --quiet "${service}"; then
        echo "Stopping ${service}..."
        systemctl stop "${service}"
    fi

    if systemctl is-enabled --quiet "${service}" 2>/dev/null; then
        echo "Disabling ${service}..."
        systemctl disable "${service}"
    fi

    if [[ -f "${service_file}" ]]; then
        echo "Removing ${service_file}..."
        rm -f "${service_file}"
    else
        echo "Service file does not exist: ${service_file}"
    fi

    systemctl daemon-reload
    systemctl reset-failed "${service}" 2>/dev/null || true

    echo "Removed ${service}."
}


# ========================================================================= Parameter parsing

parse_service_parameter() {
    local service=""

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --service)
                if [[ $# -lt 2 ]]; then
                    echo "Error: --service requires a value." >&2
                    exit 1
                fi

                service="$2"
                shift 2
                ;;

            --help|-h)
                usage
                exit 0
                ;;

            *)
                echo "Error: unknown parameter '$1'." >&2
                usage
                exit 1
                ;;
        esac
    done

    if [[ -z "${service}" ]]; then
        echo "Error: --service is required." >&2
        usage
        exit 1
    fi

    printf '%s\n' "${service}"
}


# ========================================================================= Main

main() {
    if [[ $# -eq 0 ]]; then
        usage
        exit 1
    fi

    local command="$1"
    shift

    case "${command}" in
        list)
            if [[ $# -ne 0 ]]; then
                echo "Error: list does not accept parameters." >&2
                usage
                exit 1
            fi

            list_services
            ;;

        start|stop|restart)
            local service
            service="$(parse_service_parameter "$@")"
            service_operation "${command}" "${service}"
            ;;

        remove)
            local service
            service="$(parse_service_parameter "$@")"
            remove_service "${service}"
            ;;

        daemon-reload)
            if [[ $# -ne 0 ]]; then
                echo "Error: daemon-reload does not accept parameters." >&2
                usage
                exit 1
            fi

            systemctl daemon-reload
            ;;

        --help|-h)
            usage
            ;;

        *)
            echo "Error: unknown subcommand '${command}'." >&2
            usage
            exit 1
            ;;
    esac
}


main "$@"