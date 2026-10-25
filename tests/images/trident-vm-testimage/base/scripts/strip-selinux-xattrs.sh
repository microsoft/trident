#!/bin/bash
# Strip security.selinux xattrs from every file in the cosi.
#
# Background: AZL4's base VHDX is built by the upstream Azure Linux build
# process with SELinux file labels baked in (e.g. system_u:object_r:lib_t:s0).
# Even though this test image sets `selinux: mode: disabled`, MIC does not
# strip the inherited xattrs — `mode: disabled` only controls boot-time
# SELinux state.
#
# These labels become a problem when Trident installs this cosi from the
# AZL3 MOS environment: MOS boots with selinux=1 enforcing=0, loads its
# AZL3 policy, and dracut (running inside the chroot) tries to preserve
# the AZL4 labels via cp -a. The MOS-side SELinux LSM validates the
# context being written and rejects labels not in its policy. dracut
# cascades through hundreds of "cp: setting attribute 'security.selinux':
# Permission denied" errors, eventually fatally on dracut-install's ldd
# step.
#
# Stripping the xattrs at cosi build time sidesteps this entirely:
#   - During MIC build, SELinux is not loaded inside the chroot, so
#     setfattr -x works without policy interference.
#   - During Trident install in MOS, cp -a finds no security.selinux to
#     preserve and skips the setxattr call.
#   - On first boot of the installed AZL4 OS, files get auto-relabeled if
#     SELinux is enabled (which our test config disables anyway).
#
# Once AZL4 is the install/target environment for everything (no AZL3 MOS
# bridging it), this script can be removed.

set -euo pipefail

echo "Stripping security.selinux xattrs from rootfs..."

# Walk every regular file, symlink, and directory across all filesystems
# under /. `find -xdev` would skip separately-mounted filesystems like
# `/boot` and `/var` that MIC commonly composes with — and `/boot`
# specifically carries SELinux labels on the kernel image and initramfs,
# which is exactly what dracut touches during AZL3 MOS install of the
# AZL4 cosi. So we walk the whole tree and only prune the virtual
# filesystems where xattrs don't make sense (`/proc`, `/sys`, `/dev`,
# `/run`).
#
# `setfattr` follows symlinks by default; `-h` makes it operate on the
# symlink itself, which is what we want here.
count=0
fail_count=0
while IFS= read -r -d '' f; do
    # Capture stderr so we can distinguish ENODATA ("no such attribute",
    # benign — nothing to strip) from real failures (EPERM, EOPNOTSUPP).
    err=$(setfattr -h -x security.selinux "$f" 2>&1 >/dev/null) || rc=$? && rc=${rc:-0}
    if [ "$rc" -eq 0 ]; then
        count=$((count + 1))
    elif echo "$err" | grep -qE "No such attribute|Operation not supported"; then
        : # nothing to strip, expected for files without the xattr
    else
        fail_count=$((fail_count + 1))
        echo "setfattr failed on '$f': $err" >&2
    fi
    rc=0
done < <(find / \( -path /proc -o -path /sys -o -path /dev -o -path /run \) -prune \
    -o \( -type f -o -type d -o -type l \) -print0)

echo "Stripped security.selinux from ${count} files/dirs"

if [ "$fail_count" -gt 0 ]; then
    echo "ERROR: setfattr failed (non-ENODATA) on ${fail_count} entries" >&2
    exit 1
fi

# Verify the strip actually took effect by scanning a representative set
# of paths (rootfs, /boot if present, /usr/lib/systemd, /etc). Any
# residual security.selinux means we missed something — fail loudly
# rather than warning, since the whole point of the script is to leave
# the image bare.
sentinel_dirs=( "/etc" "/usr/lib/systemd" "/usr/bin" )
if [ -d /boot ]; then
    sentinel_dirs+=( "/boot" )
fi
for d in "${sentinel_dirs[@]}"; do
    if getfattr -R -m security.selinux "$d" 2>/dev/null | grep -q security.selinux; then
        echo "ERROR: security.selinux xattr still present under '$d'" >&2
        getfattr -R -m security.selinux "$d" 2>/dev/null | head -10 >&2
        exit 1
    fi
done
