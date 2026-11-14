#!/usr/bin/env bash

set -euxo pipefail

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly DATA_DISK_MOUNT="/mnt/storage"
readonly INSTALL_CACHE_ROOT="$DATA_DISK_MOUNT/trident-cloud-agent/install"
readonly INSTALL_LOG_ROOT="$DATA_DISK_MOUNT/trident-cloud-agent/install-runs"

bash "$SCRIPT_DIR/setup-functional.sh"

sudo apt-get install -y --no-install-recommends \
    openssh-client \
    python3-cryptography \
    python3-yaml

sudo install -d \
    -m 0755 \
    -o "$(id -u)" \
    -g "$(id -g)" \
    "$INSTALL_CACHE_ROOT" \
    "$INSTALL_LOG_ROOT"

python3 -c "import cryptography, yaml"
ssh -V
