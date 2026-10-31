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
#   ./systemd.sh enable-all
#   ./systemd.sh status-all
#   ./systemd.sh restart-all
#   ./systemd.sh disable-all
#   ./systemd.sh stop-all
#   ./systemd.sh start --service rhctl-test-gateway-1.0.0
#   ./systemd.sh enable --service rhctl-test-gateway-1.0.0
#   ./systemd.sh stop --service rhctl-test-gateway-1.0.0
#   ./systemd.sh restart --service rhctl-test-gateway-1.0.0
#   ./systemd.sh remove --service rhctl-test-gateway-1.0.0
#   ./systemd.sh remove-all
#   ./systemd.sh remove-all --version 1.0.0
#   ./systemd.sh make-executable --directory /app
#   ./systemd.sh daemon-reload
#
# Subcommands:
#   list
#       List all rhctl-created systemd services.
#
#   enable-all
#       Enable all rhctl-created systemd services to start automatically at boot.
#
#   status-all
#       Show the status of all rhctl-created systemd services.
#
#   restart-all
#       Restart all rhctl-created systemd services.
#
#   disable-all
#       Disable all rhctl-created systemd services so they do not start
#       automatically at boot. Running services are not stopped. Service files
#       are not removed.
#
#   stop-all
#       Stop all rhctl-created systemd services. Services are not disabled and
#       service files are not removed.
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
#   enable
#       Enable an rhctl-created systemd service to start automatically at boot.
#
#   remove
#       Stop, disable, and permanently remove an rhctl-created service.
#
#   remove-all
#       Stop, disable, and permanently remove all rhctl-created services.
#
#   make-executable
#       Make all regular files in a directory executable.
#
#   daemon-reload
#       Reload systemd unit files after service files are created, modified, or removed.
#
# Parameters (list):
#   (none)
#
# Parameters (enable-all):
#   (none)
#
# Parameters (status-all):
#   (none)
#
# Parameters (restart-all):
#   (none)
#
# Parameters (disable-all):
#   (none)
#
# Parameters (stop-all):
#   (none)
#
# Parameters (enable):
#   --service <name>
#       Service name to enable (required).
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
# Parameters (remove-all):
#   --version <version>
#       Remove only rhctl services with the specified version suffix (optional).
#         Example version values: `1.0.0`, `1.0.1`
#
# Parameters (make-executable):
#   --directory <path>
#       Directory whose regular files should be made executable (required).
#         Example path values: `/app`, `/opt/rhctl`
#
# Parameters (daemon-reload):
#   (none)
#
# Override Parameters:
#   (none)
#
# Since : 1.0.4
# Date  : Oct 6, 2026
# ****************************************************************************************************

set -euo pipefail

readonly RHCTL_SERVICE_PREFIX="rhctl-"
readonly SYSTEMD_SERVICE_DIR="/etc/systemd/system"

readonly SCRIPT_NAME="$(basename "$0")"

readonly STATUS_FIELDS=(
    "Loaded:"
    "Active:"
    "Invocation:"
    "Main PID:"
    "Tasks:"
    "Memory:"
    "CPU:"
)


# ========================================================================= Usage

usage() {
    cat <<EOF
Usage:
  ${SCRIPT_NAME} list
  ${SCRIPT_NAME} enable-all
  ${SCRIPT_NAME} status-all
  ${SCRIPT_NAME} restart-all
  ${SCRIPT_NAME} disable-all
  ${SCRIPT_NAME} stop-all
  ${SCRIPT_NAME} start --service <name>
  ${SCRIPT_NAME} enable --service <name>
  ${SCRIPT_NAME} stop --service <name>
  ${SCRIPT_NAME} restart --service <name>
  ${SCRIPT_NAME} remove --service <name>
  ${SCRIPT_NAME} remove-all
  ${SCRIPT_NAME} remove-all --version <version>
  ${SCRIPT_NAME} make-executable --directory <path>
  ${SCRIPT_NAME} daemon-reload

Subcommands:
  list
      List all rhctl-created systemd services.

  enable-all
      Enable all rhctl-created systemd services to start automatically at boot.

  status-all
      Show the status of all rhctl-created systemd services.

  restart-all
      Restart all rhctl-created systemd services.

  disable-all
      Disable all rhctl-created systemd services so they do not start
      automatically at boot. Running services are not stopped. Service files
      are not removed.

  stop-all
      Stop all rhctl-created systemd services. Services are not disabled and
      service files are not removed.

  start
      Start an rhctl-created systemd service.

  stop
      Stop an rhctl-created systemd service.

  restart
      Restart an rhctl-created systemd service.

  enable
      Enable an rhctl-created systemd service to start automatically at boot.

  remove
      Stop, disable, and permanently remove an rhctl-created service.

  remove-all
      Stop, disable, and permanently remove all rhctl-created services.

  make-executable
      Make all regular files in a directory executable.

  daemon-reload
      Reload systemd unit files.

Parameters (enable-all):
  (none)

Parameters (status-all):
  (none)

Parameters (restart-all):
  (none)

Parameters (disable-all):
  (none)

Parameters (stop-all):
  (none)

Parameters (enable):
  --service <name>
      Service name to enable (required).

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

Parameters (remove-all):
  --version <version>
      Remove only rhctl services with the specified version suffix (optional).
        Example version values: \`1.0.0\`, \`1.0.1\`

Parameters (make-executable):
  --directory <path>
      Directory whose regular files should be made executable (required).

Parameters (daemon-reload):
  (none)
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


# ========================================================================= Service enable

enable_services() {
    local services

    services="$(list_services)"

    if [[ -z "${services}" ]]; then
        echo "No rhctl services found."
        return 0
    fi

    while IFS= read -r service; do
        [[ -n "${service}" ]] || continue

        echo "Enabling ${service}..."
        systemctl enable "${service}"
    done <<< "${services}"
}


# ========================================================================= Service enable

enable_service() {
    local service
    service="$(normalize_service_name "$1")"

    validate_service_name "${service}"

    echo "Enabling ${service}..."
    systemctl enable "${service}"

    echo "Enabled ${service}."
}


# ========================================================================= Service status

status_all_services() {
    local services
    local service
    local status_output
    local line

    services="$(list_services)"

    if [[ -z "${services}" ]]; then
        echo "No rhctl services found."
        return 0
    fi

    while IFS= read -r service; do
        [[ -n "${service}" ]] || continue

        status_output="$(
            systemctl status \
                "${service}" \
                --no-pager \
                --full \
                2>&1 || true
        )"

        while IFS= read -r line; do
            if [[ "${line}" == ●* ]]; then
                printf '%s\n' "${line}"
                continue
            fi

            for field in "${STATUS_FIELDS[@]}"; do
                if [[ "${line}" == *"${field}"* ]]; then
                    printf '%s\n' "${line}"
                    break
                fi
            done
        done <<< "${status_output}"

        echo
    done <<< "${services}"
}


# ========================================================================= Service restart

restart_all_services() {
    local services
    local service

    services="$(list_services)"

    if [[ -z "${services}" ]]; then
        echo "No rhctl services found."
        return 0
    fi

    while IFS= read -r service; do
        [[ -n "${service}" ]] || continue

        echo "Restarting ${service}..."
        systemctl restart "${service}"
    done <<< "${services}"
}


# ========================================================================= Service stop all

stop_all_services() {
    local services
    local service

    services="$(list_services)"

    if [[ -z "${services}" ]]; then
        echo "No rhctl services found."
        return 0
    fi

    while IFS= read -r service; do
        [[ -n "${service}" ]] || continue

        if systemctl is-active --quiet "${service}"; then
            echo "Stopping ${service}..."
            systemctl stop "${service}"
        fi

        systemctl reset-failed "${service}" 2>/dev/null || true

        echo "Stopped ${service}."
    done <<< "${services}"

    systemctl daemon-reload
}


# ========================================================================= Service disable all

disable_all_services() {
    local services
    local service

    services="$(list_services)"

    if [[ -z "${services}" ]]; then
        echo "No rhctl services found."
        return 0
    fi

    while IFS= read -r service; do
        [[ -n "${service}" ]] || continue

        if systemctl is-enabled --quiet "${service}" 2>/dev/null; then
            echo "Disabling ${service}..."
            systemctl disable "${service}"
        fi

        systemctl reset-failed "${service}" 2>/dev/null || true

        echo "Disabled ${service}."
    done <<< "${services}"

    systemctl daemon-reload
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


remove_all_services() {
    local version="$1"
    local services
    local service
    local service_file
    local matched=false

    services="$(list_services)"

    if [[ -z "${services}" ]]; then
        echo "No rhctl services found."
        return 0
    fi

    while IFS= read -r service; do
        [[ -n "${service}" ]] || continue

        if [[ -n "${version}" && "${service}" != *"-${version}.service" ]]; then
            continue
        fi

        matched=true
        service_file="${SYSTEMD_SERVICE_DIR}/${service}"

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

        systemctl reset-failed "${service}" 2>/dev/null || true

        echo "Removed ${service}."
    done <<< "${services}"

    if [[ "${matched}" == false ]]; then
        if [[ -n "${version}" ]]; then
            echo "No rhctl services found for version '${version}'."
        else
            echo "No rhctl services found."
        fi

        return 0
    fi

    systemctl daemon-reload
}


# ========================================================================= File permissions

make_files_executable() {
    local directory="$1"

    if [[ ! -d "${directory}" ]]; then
        echo "Error: directory does not exist: ${directory}" >&2
        return 1
    fi

    echo "Making regular files executable in ${directory}..."

    find "${directory}" \
        -maxdepth 1 \
        -type f \
        -exec chmod +x {} +

    echo "Files in ${directory} are now executable."
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


parse_remove_all_parameter() {
    local version=""

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --version)
                if [[ $# -lt 2 ]]; then
                    echo "Error: --version requires a value." >&2
                    exit 1
                fi

                version="$2"
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

    printf '%s\n' "${version}"
}


parse_directory_parameter() {
    local directory=""

    while [[ $# -gt 0 ]]; do
        case "$1" in
            --directory)
                if [[ $# -lt 2 ]]; then
                    echo "Error: --directory requires a value." >&2
                    exit 1
                fi

                directory="$2"
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

    if [[ -z "${directory}" ]]; then
        echo "Error: --directory is required." >&2
        usage
        exit 1
    fi

    printf '%s\n' "${directory}"
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

        enable-all)
            if [[ $# -ne 0 ]]; then
                echo "Error: enable-all does not accept parameters." >&2
                usage
                exit 1
            fi

            enable_services
            ;;

        status-all)
            if [[ $# -ne 0 ]]; then
                echo "Error: status-all does not accept parameters." >&2
                usage
                exit 1
            fi

            status_all_services
            ;;

        restart-all)
            if [[ $# -ne 0 ]]; then
                echo "Error: restart-all does not accept parameters." >&2
                usage
                exit 1
            fi

            restart_all_services
            ;;

        disable-all)
            if [[ $# -ne 0 ]]; then
                echo "Error: disable-all does not accept parameters." >&2
                usage
                exit 1
            fi

            disable_all_services
            ;;

        stop-all)
            if [[ $# -ne 0 ]]; then
                echo "Error: stop-all does not accept parameters." >&2
                usage
                exit 1
            fi

            stop_all_services
            ;;

        start|stop|restart)
            local service
            service="$(parse_service_parameter "$@")"
            service_operation "${command}" "${service}"
            ;;

        enable)
            local service
            service="$(parse_service_parameter "$@")"
            enable_service "${service}"
            ;;

        remove)
            local service
            service="$(parse_service_parameter "$@")"
            remove_service "${service}"
            ;;

        remove-all)
            local version
            version="$(parse_remove_all_parameter "$@")"
            remove_all_services "${version}"
            ;;

        make-executable)
            local directory
            directory="$(parse_directory_parameter "$@")"
            make_files_executable "${directory}"
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
            exit 0
            ;;

        *)
            echo "Error: unknown subcommand '${command}'." >&2
            usage
            exit 1
            ;;
    esac
}


main "$@"