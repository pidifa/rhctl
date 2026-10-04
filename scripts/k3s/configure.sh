#!/bin/bash

# ****************************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ****************************************************************************************************
# Configure the K3s Kubernetes API Server port and client kubeconfig address.
#
# If exists, overwrite — safe to re-run.
#
# Usage:
#   ./configure.sh \
#     --port 56443 \
#     --server-address 192.168.0.1
#
# Required Parameters:
#   --port <number>
#       Kubernetes API Server HTTPS listening port.
#         Example: 6443, 56443
#   --server-address <address>
#       Address used by external clients to connect to the Kubernetes API Server.
#         Example: 192.168.0.1
#
# Override Parameters:
#   RHCTL_K3S_PORT=<number>
#       Same as --port.
#   RHCTL_K3S_SERVER_ADDRESS=<address>
#       Same as --server-address.
#
# Since : 1.0.0
# Date  : Oct 5, 2026
# ****************************************************************************************************

set -e

# 权限检查
if [ "$(id -u)" -ne 0 ]; then
    echo "[ERROR] This script must be run as root."
    exit 1
fi

# ========================================================================= Parameter

CONFIG_FILE="/etc/rancher/k3s/config.yaml"
KUBECONFIG_FILE="/etc/rancher/k3s/k3s.yaml"

K3S_PORT="${RHCTL_K3S_PORT:-}"
K3S_SERVER_ADDRESS="${RHCTL_K3S_SERVER_ADDRESS:-}"

# ========================================================================= Methods

usage() {
    sed -n '/^# Usage:/,/^# Since :/p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
}

validate_port() {
    if ! [[ "${K3S_PORT}" =~ ^[0-9]+$ ]]; then
        echo "[ERROR] Invalid K3s port: ${K3S_PORT}"
        exit 1
    fi

    if [ "${K3S_PORT}" -lt 1 ] || [ "${K3S_PORT}" -gt 65535 ]; then
        echo "[ERROR] K3s port must be between 1 and 65535."
        exit 1
    fi
}

validate_server_address() {
    if [ -z "${K3S_SERVER_ADDRESS}" ]; then
        echo "[ERROR] --server-address is required."
        exit 1
    fi

    # 移除冒号限制以适配 IPv6，仅拦截斜杠和空格
    if [[ "${K3S_SERVER_ADDRESS}" =~ [/\[:space:]] ]] && ! [[ "${K3S_SERVER_ADDRESS}" =~ ^\[.*\]$ ]]; then
        echo "[ERROR] Invalid server address: ${K3S_SERVER_ADDRESS}"
        echo "[ERROR] Provide only a hostname or IP address."
        exit 1
    fi
}

# 安全更新或追加 YAML 键值配置
update_yaml_key() {
    local file="$1"
    local key="$2"
    local value="$3"

    if grep -qE "^[[:space:]]*${key}:" "${file}"; then
        sed -i -E "s#^([[:space:]]*${key}:[[:space:]]*).*#\1${value}#" "${file}"
    else
        echo "${key}: ${value}" >> "${file}"
    fi
}

# 保留现有 tls-san 并新增地址
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
        --port)
            if [ "$#" -lt 2 ]; then
                echo "[ERROR] --port requires a value."
                exit 1
            fi
            K3S_PORT="$2"
            shift 2
            ;;

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

if [ -z "${K3S_PORT}" ]; then
    echo "[ERROR] --port is required."
    exit 1
fi

validate_port
validate_server_address

# ========================================================================= Configure K3s

echo "[INFO] Configuring K3s API Server port: ${K3S_PORT}"

mkdir -p "$(dirname "${CONFIG_FILE}")"
touch "${CONFIG_FILE}"

# 安全修改配置，不覆盖已有项目
update_yaml_key "${CONFIG_FILE}" "https-listen-port" "${K3S_PORT}"
append_tls_san "${CONFIG_FILE}" "${K3S_SERVER_ADDRESS}"

# ========================================================================= Restart K3s

echo "[INFO] Restarting K3s..."
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

echo "[INFO] Updating K3s client kubeconfig..."

# 等待 k3s.yaml 生成
for i in $(seq 1 15); do
    [ -f "${KUBECONFIG_FILE}" ] && break
    sleep 1
done

if [ ! -f "${KUBECONFIG_FILE}" ]; then
    echo "[ERROR] K3s kubeconfig not found: ${KUBECONFIG_FILE}"
    exit 1
fi

# 处理 IPv6 格式方括号
FORMATTED_ADDR="${K3S_SERVER_ADDRESS}"
if [[ "${K3S_SERVER_ADDRESS}" =~ : ]] && [[ ! "${K3S_SERVER_ADDRESS}" =~ ^\[.*\]$ ]]; then
    FORMATTED_ADDR="[${K3S_SERVER_ADDRESS}]"
fi

EXPECTED_SERVER="https://${FORMATTED_ADDR}:${K3S_PORT}"

sed -i -E \
    "s#^([[:space:]]*server:[[:space:]]*)https://[^:]+:[0-9]+#\1${EXPECTED_SERVER}#" \
    "${KUBECONFIG_FILE}"

chmod 600 "${KUBECONFIG_FILE}"

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

if ! grep -qF "server: ${EXPECTED_SERVER}" "${KUBECONFIG_FILE}"; then
    echo "[ERROR] Client kubeconfig server address is incorrect."
    grep -E "^[[:space:]]*server:" "${KUBECONFIG_FILE}"
    exit 1
fi

# ========================================================================= Final output

echo
echo "[INFO] K3s configuration completed successfully."
echo "[INFO] Kubernetes API Server: ${EXPECTED_SERVER}"
echo "[INFO] Client kubeconfig: ${KUBECONFIG_FILE}"
echo
echo "[INFO] Copy ${KUBECONFIG_FILE} to the client machine."