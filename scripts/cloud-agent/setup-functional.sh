#!/usr/bin/env bash

set -euxo pipefail

readonly DATA_DISK_MOUNT="/mnt/storage"
readonly FUNCTIONAL_CACHE_ROOT="$DATA_DISK_MOUNT/trident-cloud-agent/functional"
readonly FUNCTIONAL_LOCK_DIR="/var/lib/trident-cloud-agent"

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
    xorriso \
    zstd

if ! command -v xorrisofs >/dev/null 2>&1; then
    echo "Missing xorrisofs executable (provided by xorriso)" >&2
    exit 1
fi

python3 -m pip install \
    --user \
    --disable-pip-version-check \
    -r tests/functional_tests/requirements.txt \
    "virt-firmware==26.2"

sudo usermod -a -G libvirt "$USER"
sudo install -d -m 0750 -o root -g libvirt "$FUNCTIONAL_LOCK_DIR"
sudo bash -c '
    if [[ -L "$1" || ( -e "$1" && ! -f "$1" ) ]]; then
        echo "Unsafe functional lock: $1" >&2
        exit 1
    fi
    if [[ ! -e "$1" ]]; then
        (umask 027; set -o noclobber; : > "$1")
    fi
    chown root:libvirt "$1"
    chmod 0640 "$1"
' bash "$FUNCTIONAL_LOCK_DIR/functional.lock"
sudo install -d \
    -m 0755 \
    -o "$(id -u)" \
    -g "$(id -g)" \
    "$FUNCTIONAL_CACHE_ROOT"

sudo systemctl start libvirtd.service

if [[ ! -f "$HOME/.ssh/id_rsa" ]]; then
    mkdir -p "$HOME/.ssh"
    chmod 0700 "$HOME/.ssh"
    ssh-keygen -t rsa -b 2048 -f "$HOME/.ssh/id_rsa" -q -N ""
fi

test -e /dev/kvm
LIBVIRT_DEFAULT_URI=qemu:///system sg libvirt -c "virsh list --all"
python3 -c "import assertpy, paramiko, pytest"
