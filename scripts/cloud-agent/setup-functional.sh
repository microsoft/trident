#!/usr/bin/env bash

set -euxo pipefail

readonly DATA_DISK_MOUNT="/mnt/storage"
readonly FUNCTIONAL_CACHE_ROOT="$DATA_DISK_MOUNT/trident-cloud-agent/functional"

if ! mountpoint --quiet "$DATA_DISK_MOUNT"; then
    echo "Expected runner data disk is not mounted at $DATA_DISK_MOUNT" >&2
    exit 1
fi

sudo dpkg --configure -a
sudo NEEDRESTART_MODE=a apt-get install -y --no-install-recommends \
    bc \
    bridge-utils \
    imagemagick \
    libvirt-clients \
    libvirt-daemon-system \
    openssl \
    ovmf \
    python3-bcrypt \
    python3-docker \
    python3-jinja2 \
    python3-libvirt \
    python3-netifaces \
    qemu-efi \
    qemu-kvm \
    qemu-utils \
    swtpm \
    swtpm-tools \
    virt-manager \
    zstd

python3 -m pip install \
    --user \
    --disable-pip-version-check \
    -r tests/functional_tests/requirements.txt

sudo -E python3 -m pip install \
    --disable-pip-version-check \
    "virt-firmware==26.2"
sudo sh -c 'command -v virt-fw-vars'

sudo usermod -a -G libvirt "$USER"
sudo install -d \
    -m 0755 \
    -o "$(id -u)" \
    -g "$(id -g)" \
    "$FUNCTIONAL_CACHE_ROOT"

mkdir -p "$HOME/.config/libvirt"
cat >"$HOME/.config/libvirt/libvirt.conf" <<'EOF'
uri_default = "qemu:///system"
EOF

sudo systemctl start libvirtd.service

if [[ ! -f "$HOME/.ssh/id_rsa" ]]; then
    mkdir -p "$HOME/.ssh"
    chmod 0700 "$HOME/.ssh"
    ssh-keygen -t rsa -b 2048 -f "$HOME/.ssh/id_rsa" -q -N ""
fi

test -e /dev/kvm
sg libvirt -c "virsh -c qemu:///system list --all"
python3 -c "import assertpy, paramiko, pytest"
