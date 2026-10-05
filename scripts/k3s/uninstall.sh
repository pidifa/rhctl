#!/bin/bash
# ************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ************************************************************************************
# Completely uninstall K3s and clean up all related files, services, and data.
#
# If not installed, skip — safe to re-run.
#
# Usage:
#   ./uninstall.sh
#   ./uninstall.sh --force
#
# Required Parameters:
#   (none)
#
# Optional Parameters:
#   --force
#       Skip confirmation prompt and force removal (default: off).
#
# Override Parameters:
#   RHCTL_K3S_FORCE=1
#       Same as `--force`.
#
# Since : 1.0.4
# Date  : Oct 5, 2026
# ************************************************************************************

set -e

# Check root privileges
if [ "$(id -u)" -ne 0 ]; then
    echo "[ERROR] This script must be run as root."
    exit 1
fi

# ========================================================================= Parameter

FORCE="${RHCTL_K3S_FORCE:-0}"

# ========================================================================= Methods

usage() {
    sed -n '/^# Usage:/,/^# Since :/p' "$0" | sed 's/^# \{0,1\}//'
    exit 1
}

# ========================================================================= Parse Parameters

while [ "$#" -gt 0 ]; do
    case "$1" in
        --force)
            FORCE=1
            shift
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

# ========================================================================= Check if K3s is installed

K3S_UNINSTALL="/usr/local/bin/k3s-uninstall.sh"
K3S_AGENT_UNINSTALL="/usr/local/bin/k3s-agent-uninstall.sh"
K3S_BINARY="/usr/local/bin/k3s"
K3S_SERVICE="/etc/systemd/system/k3s.service"
K3S_AGENT_SERVICE="/etc/systemd/system/k3s-agent.service"

if [ ! -f "${K3S_UNINSTALL}" ] && \
   [ ! -f "${K3S_AGENT_UNINSTALL}" ] && \
   [ ! -f "${K3S_BINARY}" ] && \
   [ ! -f "${K3S_SERVICE}" ] && \
   [ ! -f "${K3S_AGENT_SERVICE}" ] && \
   [ ! -d "/var/lib/rancher/k3s" ] && \
   [ ! -d "/etc/rancher/k3s" ]; then
    echo "[INFO] K3s is not installed. Nothing to do."
    exit 0
fi

echo "[WARN] This will completely remove K3s, all cluster data, and related files."
echo "[WARN] This action cannot be undone."

# ========================================================================= Stop services

echo "[INFO] Stopping K3s services..."

if systemctl list-unit-files | grep -q '^k3s.service'; then
    systemctl stop k3s 2>/dev/null || true
    systemctl disable k3s 2>/dev/null || true
fi

if systemctl list-unit-files | grep -q '^k3s-agent.service'; then
    systemctl stop k3s-agent 2>/dev/null || true
    systemctl disable k3s-agent 2>/dev/null || true
fi

# Kill any remaining k3s / containerd-shim processes
pkill -9 -f 'k3s-server|k3s-agent|k3s ' 2>/dev/null || true
pkill -9 -f 'containerd-shim-runc-v2.*k3s' 2>/dev/null || true

# ========================================================================= Official uninstall scripts

echo "[INFO] Running official K3s uninstall scripts (if present)..."

if [ -x "${K3S_UNINSTALL}" ]; then
    echo "[INFO] Executing ${K3S_UNINSTALL}"
    "${K3S_UNINSTALL}" || true
fi

if [ -x "${K3S_AGENT_UNINSTALL}" ]; then
    echo "[INFO] Executing ${K3S_AGENT_UNINSTALL}"
    "${K3S_AGENT_UNINSTALL}" || true
fi

# ========================================================================= Clean remaining files

echo "[INFO] Removing remaining K3s files and directories..."

# Binaries and helpers
rm -f /usr/local/bin/k3s \
      /usr/local/bin/k3s-uninstall.sh \
      /usr/local/bin/k3s-agent-uninstall.sh \
      /usr/local/bin/kubectl \
      /usr/local/bin/crictl \
      /usr/local/bin/ctr 2>/dev/null || true

# Systemd units
rm -f /etc/systemd/system/k3s.service \
      /etc/systemd/system/k3s-agent.service \
      /etc/systemd/system/k3s.service.env \
      /etc/systemd/system/k3s-agent.service.env 2>/dev/null || true

rm -rf /etc/systemd/system/k3s.service.d \
       /etc/systemd/system/k3s-agent.service.d 2>/dev/null || true

systemctl daemon-reload 2>/dev/null || true
systemctl reset-failed k3s 2>/dev/null || true
systemctl reset-failed k3s-agent 2>/dev/null || true

# Configuration and data
rm -rf /etc/rancher/k3s \
       /var/lib/rancher/k3s \
       /var/lib/kubelet \
       /var/lib/cni \
       /etc/cni \
       /opt/cni \
       /run/k3s \
       /run/flannel \
       /var/log/pods \
       /var/log/containers 2>/dev/null || true

# kubeconfig in user home (common locations)
rm -f /root/.kube/config 2>/dev/null || true
if [ -n "${SUDO_USER}" ] && [ -d "/home/${SUDO_USER}" ]; then
    rm -f "/home/${SUDO_USER}/.kube/config" 2>/dev/null || true
fi

# iptables / network leftovers (best effort)
iptables -F 2>/dev/null || true
iptables -t nat -F 2>/dev/null || true
iptables -t mangle -F 2>/dev/null || true
ip6tables -F 2>/dev/null || true
ip6tables -t nat -F 2>/dev/null || true

# ========================================================================= Final output

echo
echo "[INFO] K3s has been completely uninstalled."
echo "[INFO] All services, binaries, configuration, and cluster data have been removed."
echo