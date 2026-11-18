#!/bin/bash
set -euo pipefail

variant=${1:?variant is required}
phase=${2:?phase is required}
case "$variant" in standard|mshv) ;; *) exit 1 ;; esac
case "$phase" in base|update) ;; *) exit 1 ;; esac

mkdir -p /etc/systemd/network /etc/ssh/sshd_config.d /var/lib/trident /oem
cat > /etc/systemd/network/00-synthetic-acl.network <<'NETWORK'
[Match]
Type=ether

[Network]
LinkLocalAddressing=no
IPv6AcceptRA=no
DHCP=ipv4
NETWORK
cat > /etc/ssh/sshd_config.d/00-synthetic-acl.conf <<'SSH'
PermitRootLogin prohibit-password
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
SSH
# AZL's sshd_config does not include drop-ins, and OpenSSH uses the first value.
ssh_include='Include /etc/ssh/sshd_config.d/*.conf'
if [[ "$(head -n 1 /etc/ssh/sshd_config)" != "$ssh_include" ]]; then
    sed -i "1i$ssh_include" /etc/ssh/sshd_config
fi
passwd -d root
# Tests must issue commit explicitly, without the normal boot-time commit unit.
systemctl mask trident.service trident-acl-agent.service
systemctl enable systemd-networkd sshd tridentd.socket

# Keep the AZL package manager's release detection intact until all packages
# and initramfs files are prepared. These are synthetic ACL test images only.
dracut --force --regenerate-all
test -x /usr/lib/systemd/system-generators/systemd-veritysetup-generator
test -x /usr/lib/systemd/systemd-veritysetup
test -x /usr/bin/trident
cat > /usr/lib/os-release <<EOF
NAME="Microsoft Azure Container Linux"
ID=azurelinux
ID_LIKE=flatcar
VARIANT="Azure Container Linux"
VARIANT_ID=azurecontainerlinux
VERSION="3.0 Synthetic ACL ${phase}"
VERSION_ID=3.0
BUILD_ID=synthetic-${variant}-${phase}
PRETTY_NAME="Synthetic ACL ${variant} ${phase} (test only)"
EOF
ln -sfn ../usr/lib/os-release /etc/os-release
printf '%s\n' "$phase" > /usr/lib/synthetic-acl-generation
rm -f /etc/machine-id
touch /etc/machine-id
ssh-keygen -A
sshd -t
effective_sshd=$(sshd -T -C user=root,host=localhost,addr=127.0.0.1)
printf '%s\n' "$effective_sshd" | grep -Ex 'permitrootlogin (prohibit-password|without-password)' > /dev/null
printf '%s\n' "$effective_sshd" | grep -Fx 'pubkeyauthentication yes' > /dev/null
printf '%s\n' "$effective_sshd" | grep -Fx 'passwordauthentication no' > /dev/null
printf '%s\n' "$effective_sshd" | grep -Fx 'kbdinteractiveauthentication no' > /dev/null
for initrd in /boot/initramfs-*.img; do
    lsinitrd "$initrd" | grep systemd-veritysetup > /dev/null
    lsinitrd "$initrd" | grep hv_storvsc > /dev/null ||
        grep hv_storvsc /usr/lib/modules/*/modules.builtin > /dev/null
done
