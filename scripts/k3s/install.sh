#!/bin/bash

# ****************************************************************************************************
# Copyright (C) 2022-2026 rhctl Contributors
#
# SPDX-License-Identifier: Apache-2.0
# ****************************************************************************************************
# Install K3s without Traefik and configure kubectl.
#
# If exists, skip — safe to re-run.
#
# Usage:
#   ./install.sh
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
# Since : 1.0.4
# Date  : Oct 4, 2026
# ****************************************************************************************************

set -e

# ========================================================================= Parameter

K3S_BIN="/usr/local/bin/k3s"
KUBECTL_BIN="/usr/local/bin/kubectl"

# ========================================================================= Install K3s

if [ -x "${K3S_BIN}" ]; then
    echo "[INFO] K3s already installed, skipping."
else
    echo "[INFO] Installing K3s without Traefik..."

    curl -sfL https://get.k3s.io | sh -s - --disable=traefik
fi

# ========================================================================= Configure kubectl

echo "[INFO] Configuring kubectl symlink..."

# Overwrite kubectl
ln -sfn "${K3S_BIN}" "${KUBECTL_BIN}" 

# ========================================================================= Verify K3s

echo "[INFO] Verifying K3s..."

k3s --version

# ========================================================================= Verify kubectl

echo "[INFO] Verifying kubectl..."

kubectl get nodes

# ========================================================================= Final output

echo
echo "[INFO] K3s installation completed."
echo "[INFO] Traefik is disabled."
echo "[INFO] kubectl is available at ${KUBECTL_BIN}."