#!/bin/bash
set -euo pipefail

phase=${1:?phase required}
variant=${2:?variant required}
target_uuid=${3:?target UUID required}
target_hash=${4:?target verity root hash required}
expected_trident_sha256=${5:-}
slot=${6:?expected slot required}
generation=${7:?expected generation required}
previous=${8:?previous evidence phase required}
[[ -z "$expected_trident_sha256" || "$expected_trident_sha256" =~ ^[a-fA-F0-9]{64}$ ]]
[[ "$phase" =~ ^[a-z0-9-]+$ && "$previous" =~ ^[a-z0-9-]+$ ]]
[[ "$variant" =~ ^(standard|mshv)$ ]]
[[ "$target_uuid" =~ ^[a-f0-9-]{36}$ ]]
[[ "$target_hash" =~ ^[a-f0-9]{64}$ ]]
[[ "$slot" =~ ^[ab]$ && "$generation" =~ ^(base|update)$ ]]
partition=sda2
slot_dir=AZLA
other_dir=AZLB
if [[ "$slot" = b ]]; then
    partition=sda4
    slot_dir=AZLB
    other_dir=AZLA
fi
out="/var/lib/trident/test-evidence/$phase"
reference="/var/lib/trident/test-evidence/$previous"
baseline=/var/lib/trident/test-evidence/baseline
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
blkid -s UUID -o value "/dev/$partition" > "$out/usr-uuid"
awk '/root hash:/ {print $NF}' "$out/verity" > "$out/usr-roothash"
grep -Eq '^[a-f0-9]{64}$' "$out/usr-roothash"
lsblk -s -rno NAME /dev/mapper/usr | grep -qx "$partition"
grep -qx "$generation" /usr/lib/synthetic-acl-generation
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

for tree in BOOT AZLA AZLB; do
    if [[ -d "/boot/EFI/$tree" ]]; then
        (
            cd "/boot/EFI/$tree"
            find . -type f -print0 | sort -z | xargs -0 -r sha256sum
        ) > "$out/$tree.sha256"
    fi
done

if [[ "$phase" = baseline ]]; then
    test -s "$out/BOOT.sha256"
    exit 0
fi

trident --verbosity error get status > "$out/status"
if grep -q '^lastError:' "$out/status"; then exit 1; fi
cmp "$baseline/trident.sha256" "$out/trident.sha256"
if [[ "$generation" = base ]]; then
    cmp "$baseline/usr-uuid" "$out/usr-uuid"
    cmp "$baseline/usr-roothash" "$out/usr-roothash"
else
    test "$(cat "$out/usr-uuid")" = "$target_uuid"
    test "$(cat "$out/usr-roothash")" = "$target_hash"
fi
reference_slot=$slot_dir
if [[ "$previous" = baseline ]]; then reference_slot=BOOT; fi
cmp "$reference/$reference_slot.sha256" "$out/$slot_dir.sha256"
if [[ "$phase" = *-staged ]]; then
    grep -qx 'servicingState: ab-update-staged' "$out/status"
    grep -qx "abActiveVolume: volume-$slot" "$out/status"
    cmp "$reference/boot-id" "$out/boot-id"
    cmp "$reference/BOOT.sha256" "$out/BOOT.sha256"
    test -s "$out/$other_dir.sha256"
    exit 0
fi

cmp "$reference/$other_dir.sha256" "$out/$other_dir.sha256"
if [[ "$variant" = mshv ]]; then
    grep -iE "^[[:space:]]*File: .*EFI/$slot_dir/second\\.efi$" "$out/bootctl"
    for file in bootx64.efi grubx64.efi second.efi HvLoader.efi lxhvloader.dll \
        Windows/System32/hvix64.exe Windows/System32/hvax64.exe Windows/System32/kdstub.dll; do
        test -s "/boot/EFI/$slot_dir/$file"
    done
else
    grep -iE "^[[:space:]]*File: .*EFI/$slot_dir/[^/]+\\.efi$" "$out/bootctl"
    test ! -e "/boot/EFI/$slot_dir/HvLoader.efi"
fi
cmp "$out/$slot_dir.sha256" "$out/BOOT.sha256"
