#!/bin/bash
set -euo pipefail

phase=${1:?phase required}
variant=${2:?variant required}
target_uuid=${3:?target UUID required}
target_hash=${4:?target verity root hash required}
expected_trident_sha256=${5:-}
[[ -z "$expected_trident_sha256" || "$expected_trident_sha256" =~ ^[a-fA-F0-9]{64}$ ]]
[[ "$phase" =~ ^(baseline|staged|updated|committed|persisted)$ ]]
[[ "$variant" =~ ^(standard|mshv)$ ]]
[[ "$target_uuid" =~ ^[a-f0-9-]{36}$ ]]
[[ "$target_hash" =~ ^[a-f0-9]{64}$ ]]
out="/var/lib/trident/test-evidence/$phase"
mkdir -p "$out"

cat /proc/sys/kernel/random/boot_id > "$out/boot-id"
cat /proc/cmdline > "$out/cmdline"
cat /usr/lib/os-release > "$out/os-release"
sha256sum /usr/bin/trident > "$out/trident.sha256"
uname -r > "$out/kernel"
findmnt /usr > "$out/usr-mount"
veritysetup status usr > "$out/verity"
lsblk -o NAME,SIZE,FSTYPE,PARTLABEL,UUID,MOUNTPOINTS > "$out/partitions"
bootctl status --no-pager > "$out/bootctl" 2>&1
journalctl -b --no-pager > "$out/journal"
grep -qx 'VARIANT_ID=azurecontainerlinux' "$out/os-release"
grep -qi 'verity' "$out/verity"
findmnt -rn -o SOURCE /usr | grep -qx /dev/mapper/usr
findmnt -rn -o OPTIONS /usr | grep -Eq '(^|,)ro(,|$)'
if [[ -n "$expected_trident_sha256" ]]; then
    test "$(cut -d ' ' -f 1 "$out/trident.sha256")" = "${expected_trident_sha256,,}"
fi
if [[ "$variant" = mshv ]]; then
    test -c /dev/mshv
    grep -q 'mshv' "$out/kernel"
else
    if grep -q 'mshv' "$out/kernel"; then exit 1; fi
    test ! -e /dev/mshv
fi

for slot in BOOT AZLA AZLB; do
    if [[ -d "/boot/EFI/$slot" ]]; then
        (
            cd "/boot/EFI/$slot"
            find . -type f -print0 | sort -z | xargs -0 -r sha256sum
        ) > "$out/$slot.sha256"
    fi
done

if [[ "$phase" = baseline ]]; then
    lsblk -s -rno NAME /dev/mapper/usr | grep -qx sda2
    test -s "$out/BOOT.sha256"
    exit 0
fi

trident --verbosity error get status > "$out/status"
if grep -q '^lastError:' "$out/status"; then exit 1; fi
cmp /var/lib/trident/test-evidence/baseline/trident.sha256 "$out/trident.sha256"
cmp /var/lib/trident/test-evidence/baseline/BOOT.sha256 "$out/AZLA.sha256"
if [[ "$phase" = staged ]]; then
    grep -qx 'servicingState: ab-update-staged' "$out/status"
    cmp /var/lib/trident/test-evidence/baseline/boot-id "$out/boot-id"
    test -s "$out/AZLB.sha256"
    lsblk -s -rno NAME /dev/mapper/usr | grep -qx sda2
    exit 0
fi

# ACL adoption selects slots by device identity, not by an acl.slot token.
test "$(blkid -s UUID -o value /dev/sda4)" = "$target_uuid"
lsblk -s -rno NAME /dev/mapper/usr | grep -qx sda4
grep -qi "$target_hash" "$out/verity"
cmp /var/lib/trident/test-evidence/staged/AZLB.sha256 "$out/AZLB.sha256"
if [[ "$variant" = mshv ]]; then
    grep -iE '^[[:space:]]*File: .*EFI/AZLB/second\.efi$' "$out/bootctl"
    for file in bootx64.efi grubx64.efi second.efi HvLoader.efi lxhvloader.dll \
        Windows/System32/hvix64.exe Windows/System32/hvax64.exe Windows/System32/kdstub.dll; do
        test -s "/boot/EFI/AZLB/$file"
    done
else
    grep -iE '^[[:space:]]*File: .*EFI/AZLB/[^/]+\.efi$' "$out/bootctl"
    test ! -e /boot/EFI/AZLB/HvLoader.efi
fi
if [[ "$phase" = updated ]]; then
    grep -qx 'servicingState: ab-update-finalized' "$out/status"
    exit 0
fi
grep -qx 'servicingState: provisioned' "$out/status"
grep -qx 'abActiveVolume: volume-b' "$out/status"
cmp "$out/AZLB.sha256" "$out/BOOT.sha256"
