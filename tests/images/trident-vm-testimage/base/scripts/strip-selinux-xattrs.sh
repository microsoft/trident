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

count=0
# Walk every regular file, symlink, and directory; setfattr -h follows
# symlinks by default so use -h to operate on the link itself.
# Use a process-substitution + while loop to keep the script bash-portable
# and avoid argument-list-too-long issues from xargs.
while IFS= read -r -d '' f; do
    if setfattr -h -x security.selinux "$f" 2>/dev/null; then
        count=$((count + 1))
    fi
done < <(find / -xdev \( -type f -o -type d -o -type l \) -print0)

echo "Stripped security.selinux from ${count} files/dirs"

# Quick sanity check: confirm at least the kbd files (which were the most
# visible offender in the original dracut failure) are now bare.
if ls /usr/lib/kbd/keymaps/legacy/i386/qwerty/fa.map.gz >/dev/null 2>&1; then
    remaining=$(getfattr -m security -d \
        /usr/lib/kbd/keymaps/legacy/i386/qwerty/fa.map.gz 2>&1 || true)
    if [ -n "$remaining" ] && echo "$remaining" | grep -q security.selinux; then
        echo "WARNING: security.selinux still present on sample file:"
        echo "$remaining"
    fi
fi
