#!/usr/bin/env bash

set -euxo pipefail

readonly DATA_DISK_MOUNT="/mnt/storage"

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
    virt-manager \
    zstd

python3 -m pip install \
    --user \
    --disable-pip-version-check \
    -r tests/functional_tests/requirements.txt \
    "virt-firmware==26.2"

sudo usermod -a -G libvirt "$USER"

mkdir -p "$HOME/.config/libvirt"
cat >"$HOME/.config/libvirt/libvirt.conf" <<'EOF'
uri_default = "qemu:///system"
EOF

sudo mkdir -p /etc/systemd/system/libvirtd.socket.d
cat <<'EOF' | sudo tee /etc/systemd/system/libvirtd.socket.d/mode.conf >/dev/null
[Socket]
SocketMode=0666
EOF

sudo systemctl daemon-reload
sudo systemctl restart libvirtd.socket
sudo systemctl restart libvirtd.service

if [[ ! -f "$HOME/.ssh/id_rsa" ]]; then
    mkdir -p "$HOME/.ssh"
    chmod 0700 "$HOME/.ssh"
    ssh-keygen -t rsa -b 2048 -f "$HOME/.ssh/id_rsa" -q -N ""
fi

test -e /dev/kvm
virsh -c qemu:///system list --all
python3 -c "import assertpy, paramiko, pytest"
