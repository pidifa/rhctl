#!/bin/bash

# ****************************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ****************************************************************************************************
# Configure the K3s client kubeconfig address (external access).
#
# This script does NOT change the K3s API Server listening port.
# The API Server always uses the default port 6443.
#
# If exists, overwrite — safe to re-run.
#
# Usage:
#   ./configure.sh \
#     --server-address 192.168.0.1
#
# Required Parameters:
#   --server-address <address>
#       Address used by external clients to connect to the Kubernetes API Server.
#         Example: 192.168.0.1
#
# Override Parameters:
#   RHCTL_K3S_SERVER_ADDRESS=<address>
#       Same as --server-address.
#
# Since : 1.0.4
# Date  : Oct 5, 2026
# ****************************************************************************************************

set -e

# Check root privileges
if [ "$(id -u)" -ne 0 ]; then
    echo "[ERROR] This script must be run as root."
    exit 1
fi

# ========================================================================= Parameter

CONFIG_FILE="/etc/rancher/k3s/config.yaml"
KUBECONFIG_FILE="/etc/rancher/k3s/k3s.yaml"
EXTERNAL_KUBECONFIG="/etc/rancher/k3s/k3s-external.yaml"

# Fixed default API Server port
K3S_PORT="6443"

K3S_SERVER_ADDRESS="${RHCTL_K3S_SERVER_ADDRESS:-}"

# ========================================================================= Methods

usage() {
    sed -n '/^# Usage:/,/^# Since :/p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
}

validate_server_address() {
    if [ -z "${K3S_SERVER_ADDRESS}" ]; then
        echo "[ERROR] --server-address is required."
        exit 1
    fi

    # Allow IPv6 (with or without brackets). Block only slashes and spaces.
    if [[ "${K3S_SERVER_ADDRESS}" =~ [/\[:space:]] ]] && ! [[ "${K3S_SERVER_ADDRESS}" =~ ^\[.*\]$ ]]; then
        echo "[ERROR] Invalid server address: ${K3S_SERVER_ADDRESS}"
        echo "[ERROR] Provide only a hostname or IP address."
        exit 1
    fi
}

# Keep existing tls-san entries and append the new address if missing
append_tls_san() {
    local file="$1"
    local address="$2"

    if ! grep -qF "${address}" "${file}" 2>/dev/null; then
        if ! grep -qE "^[[:space:]]*tls-san:" "${file}"; then
            echo "tls-san:" >> "${file}"
        fi
        echo "  - \"${address}\"" >> "${file}"
    fi
}

# ========================================================================= Parse Parameters

while [ "$#" -gt 0 ]; do
    case "$1" in
        --server-address)
            if [ "$#" -lt 2 ]; then
                echo "[ERROR] --server-address requires a value."
                exit 1
            fi
            K3S_SERVER_ADDRESS="$2"
            shift 2
            ;;

        --help|-h)
            usage
            ;;

        *)
            echo "[ERROR] Unknown parameter: $1"
            usage
            ;;
    esac
done

validate_server_address

# ========================================================================= Configure K3s

echo "[INFO] Configuring K3s with server address: ${K3S_SERVER_ADDRESS}"
echo "[INFO] API Server port remains default: ${K3S_PORT}"

mkdir -p "$(dirname "${CONFIG_FILE}")"
touch "${CONFIG_FILE}"

# Add the external address to tls-san
append_tls_san "${CONFIG_FILE}" "${K3S_SERVER_ADDRESS}"

# ========================================================================= Restart K3s (needed for tls-san to take effect)

echo "[INFO] Restarting K3s to apply tls-san..."
systemctl restart k3s

# ========================================================================= Verify K3s

echo "[INFO] Waiting for K3s service..."

for i in $(seq 1 30); do
    if systemctl is-active --quiet k3s; then
        break
    fi
    sleep 1
done

if ! systemctl is-active --quiet k3s; then
    echo "[ERROR] K3s failed to start."
    systemctl status k3s --no-pager
    exit 1
fi

echo "[INFO] K3s service is running."

# ========================================================================= Configure Client Kubeconfig

echo "[INFO] Generating external K3s client kubeconfig..."

# Wait for the original k3s.yaml to be generated
for i in $(seq 1 15); do
    [ -f "${KUBECONFIG_FILE}" ] && break
    sleep 1
done

if [ ! -f "${KUBECONFIG_FILE}" ]; then
    echo "[ERROR] K3s kubeconfig not found: ${KUBECONFIG_FILE}"
    exit 1
fi

# Duplicate the file, because the original is frequently overwritten back to 127.0.0.1 by the K3s daemon
cp "${KUBECONFIG_FILE}" "${EXTERNAL_KUBECONFIG}"

# Handle IPv6 format with square brackets
FORMATTED_ADDR="${K3S_SERVER_ADDRESS}"
if [[ "${K3S_SERVER_ADDRESS}" =~ : ]] && [[ ! "${K3S_SERVER_ADDRESS}" =~ ^\[.*\]$ ]]; then
    FORMATTED_ADDR="[${K3S_SERVER_ADDRESS}]"
fi

EXPECTED_SERVER="https://${FORMATTED_ADDR}:${K3S_PORT}"

# Replace the server address in the external kubeconfig
sed -i -E \
    "s#^([[:space:]]*server:[[:space:]]*)https://.*:[0-9]+#\1${EXPECTED_SERVER}#" \
    "${EXTERNAL_KUBECONFIG}"

chmod 600 "${EXTERNAL_KUBECONFIG}"

# ========================================================================= Verify Port

echo "[INFO] Verifying K3s API Server port listening..."

PORT_ACTIVE=false
for i in $(seq 1 20); do
    if ss -lnt | grep -qE ":${K3S_PORT}([[:space:]]|$)"; then
        PORT_ACTIVE=true
        break
    fi
    sleep 1
done

if [ "${PORT_ACTIVE}" = "false" ]; then
    echo "[ERROR] K3s API Server is not listening on port ${K3S_PORT} after waiting."
    exit 1
fi

# ========================================================================= Verify Client Kubeconfig

echo "[INFO] Verifying client kubeconfig..."

if ! grep -qF "server: ${EXPECTED_SERVER}" "${EXTERNAL_KUBECONFIG}"; then
    echo "[ERROR] Client kubeconfig server address is incorrect."
    grep -E "^[[:space:]]*server:" "${EXTERNAL_KUBECONFIG}"
    exit 1
fi

# ========================================================================= Final output

echo
echo "[INFO] K3s configuration completed successfully."
echo "[INFO] Kubernetes API Server: ${EXPECTED_SERVER}"
echo "[INFO] Local Kubeconfig (internal): ${KUBECONFIG_FILE}"
echo "[INFO] Client Kubeconfig (external): ${EXTERNAL_KUBECONFIG}"
echo
echo "[INFO] Copy ${EXTERNAL_KUBECONFIG} to the client machine."