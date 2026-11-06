#!/usr/bin/env bash

set -euxo pipefail

readonly SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

bash "$SCRIPT_DIR/setup-functional.sh"

sudo apt-get install -y --no-install-recommends \
    openssh-client \
    python3-cryptography \
    python3-yaml

python3 -c "import cryptography, yaml"
ssh -V
