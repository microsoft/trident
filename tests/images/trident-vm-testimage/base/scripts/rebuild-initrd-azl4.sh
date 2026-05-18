#!/bin/bash
# Regenerate initrd with --no-hostonly so all storage drivers are
# included, not just the ones MIC's build environment happens to need.
#
# Why: storm-trident's rollback test (tools/storm/utils/vm/qemu/qemu.go)
# attaches the qcow2 to a virt-install VM with `bus=sata`. MIC builds
# the qcow2 in a virtio-backed environment, so dracut's default
# hostonly mode produces an initramfs with only virtio drivers. On a
# SATA-backed boot, the initramfs can't find the root partition by
# UUID and systemd hangs forever waiting for /dev/disk/by-uuid/<root>.
#
# Rebuilding with --no-hostonly bakes in ahci, ata_piix, sata_sil, etc.
# along with virtio so the same qcow2 boots regardless of the bus type
# the consumer chooses.
#
# Runs inside the MIC chroot where /sys and /proc are bind-mounted but
# the host's SELinux is not loaded (MIC strips that), so dracut's
# cp -a doesn't hit the security.selinux setxattr issue that bites in
# AZL3 MOS during install (see strip-selinux-xattrs.sh for the parallel
# write-up).

set -euo pipefail

# Find the kernel version installed in this image; there should be
# exactly one.
KVER=$(ls /usr/lib/modules | head -1)
if [ -z "$KVER" ]; then
    echo "ERROR: no kernel modules dir under /usr/lib/modules"
    exit 1
fi
echo "Regenerating initramfs for kernel $KVER with --no-hostonly"

dracut \
    --no-hostonly \
    --add-drivers "ahci ata_piix sata_sil sata_nv sata_via sd_mod" \
    --force \
    --kver "$KVER"

echo "Regenerated initramfs:"
ls -lh /boot/initramfs-*
