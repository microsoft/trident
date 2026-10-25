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

# Find the kernel version installed in this image. We require exactly
# one — `ls | head -1` would silently pick the wrong one if any future
# AZL4 variant ships multiple (kernel + kernel-hyperv, extramodules-*,
# etc.). Fail loudly rather than generate an initramfs for the wrong
# kernel: the failure mode of that misstep is "boot hangs waiting for
# /dev/disk/by-uuid/<root>", which is the exact bug this script is
# meant to prevent.
# nullglob so an empty/missing modules dir yields a zero-length array
# (reaching the 0) arm below) instead of the literal glob pattern.
shopt -s nullglob
KVERS=( /usr/lib/modules/* )
case ${#KVERS[@]} in
    0)
        echo "ERROR: no kernel modules dir under /usr/lib/modules" >&2
        exit 1
        ;;
    1)
        KVER=$(basename "${KVERS[0]}")
        ;;
    *)
        echo "ERROR: expected exactly one kernel under /usr/lib/modules, found:" >&2
        printf '  %s\n' "${KVERS[@]}" >&2
        exit 1
        ;;
esac
echo "Regenerating initramfs for kernel $KVER with --no-hostonly"

# `--no-hostonly` includes all storage modules; `--no-hostonly-cmdline`
# prevents dracut from baking the build-host's /proc/cmdline parameters
# into the initramfs (which would fight the qcow2's grub cmdline at
# runtime); `--reproducible` keeps the output bit-stable across builds
# so we can detect spurious regenerations.
dracut \
    --no-hostonly \
    --no-hostonly-cmdline \
    --reproducible \
    --add-drivers "ahci ata_piix sata_sil sata_nv sata_via sd_mod" \
    --force \
    --kver "$KVER"

echo "Regenerated initramfs:"
ls -lh /boot/initramfs-*
