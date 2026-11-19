#!/bin/bash
# Package already-built native ACL images for the Storm servicing scenario.
set -euo pipefail
export LC_ALL=C

if [[ $# != 5 || ( $1 != standard && $1 != mshv ) ]]; then
    echo "Usage: $0 standard|mshv BASE.qcow2 UPDATE.cosi PRIVATE_KEY OUTPUT_DIR" >&2
    exit 1
fi
variant=$1
base=$(realpath -e -- "$2")
update=$(realpath -e -- "$3")
key=$(realpath -e -- "$4")
output=$(realpath -m -- "$5")
root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../../.." && pwd)
mkdir -p "$output"
[[ -z $(find "$output" -mindepth 1 -maxdepth 1 -print -quit) ]] || {
    echo 'Output directory must be empty' >&2
    exit 1
}
scratch=$(mktemp -d "$output/.packaging.XXXXXX")
trap 'rm -rf -- "$scratch"' EXIT

source_metadata="${base%.*}.source.json"
cmp -- "$source_metadata" "${update%.*}.source.json" || {
    echo 'Base and update must have matching ACL-T source provenance' >&2
    exit 1
}
jq -e 'select(.pipelineId == 5304 and .repository == "acl-pipelines"
    and .reason == "schedule" and .status == "completed" and .result == "succeeded"
    and .sourceBranch == "refs/heads/main"
    and .repositories.self.refName == "refs/heads/main"
    and .repositories.azure_container_linux.refName == "refs/heads/aclmain"
    and (.runId | type == "number" and . > 0 and floor == .)
    and (.sha256 | test("^[a-f0-9]{64}$")))' "$source_metadata" > "$scratch/source.json"

tar -xOf "$update" metadata.json > "$scratch/original.json"
jq -e --arg variant "$variant" '
    if .version == "1.2" and .osArch == "x86_64"
        and .bootloader.type == "systemd-boot"
        and (.bootloader.systemdBoot.entries | length == 1)
        and ((.bootloader.systemdBoot.entries[0].kernel | contains("mshv")) == ($variant == "mshv"))
        and ([.images[] | select(.mountPoint == "/usr" and .fsType == "btrfs"
            and (.verity.roothash | test("^[a-f0-9]{64}$")))] | length == 1)
    then if $variant == "mshv"
        then .version = "1.3" | .bootloader.type = "efitreeloader-mshv-systemd-boot"
        else . end
    else error("Expected native ACL-T COSI with one verity-backed Btrfs /usr and UKI")
    end' "$scratch/original.json" > "$scratch/metadata.json"

usr=$(jq -r '.images[] | select(.mountPoint == "/usr") | .image.path' "$scratch/metadata.json")
[[ $usr =~ ^images/[a-zA-Z0-9._-]+$ ]]
tar -xOf "$update" "$usr" > "$scratch/usr.zst"
expected=$(jq -r '.images[] | select(.mountPoint == "/usr") | .image.sha384' "$scratch/metadata.json")
printf '%s  %s\n' "$expected" "$scratch/usr.zst" | sha384sum --check -
zstd -dq "$scratch/usr.zst" -o "$scratch/usr.raw"
update_binary=$(guestfish --ro -a "$scratch/usr.raw" run : mount-ro /dev/sda / : checksum sha256 /bin/trident)
base_binary=$(guestfish --ro -a "$base" run : mount-ro /dev/sda2 / : checksum sha256 /bin/trident)
[[ $base_binary =~ ^[a-f0-9]{64}$ && $base_binary == "$update_binary" ]] || {
    echo 'Base and update must contain identical candidate Trident binaries' >&2
    exit 1
}
base_uuid=$(guestfish --ro -a "$base" run : vfs-uuid /dev/sda2)
update_uuid=$(jq -r '.images[] | select(.mountPoint == "/usr") | .fsUuid' "$scratch/metadata.json")
[[ $base_uuid != "$update_uuid" ]] || {
    echo 'Base and update must have distinct /usr UUIDs' >&2
    exit 1
}
for image in "$base" "$scratch/usr.raw"; do
    device=/dev/sda2
    [[ $image == "$base" ]] || device=/dev/sda
    guestfish --ro -a "$image" run : mount-ro "$device" / : cat /lib/os-release |
        grep -Fx 'VARIANT_ID=azurecontainerlinux'
done
public=$(ssh-keygen -y -P '' -f "$key" | awk '{ print $1, $2 }')
authorized=$(guestfish --ro -a "$base" run : mount-ro /dev/sda7 / : cat /home/testuser/.ssh/authorized_keys)
printf '%s\n' "$authorized" | awk '{ print $1, $2 }' | grep -Fxq "$public" || {
    echo 'Private key does not match the base image testuser key' >&2
    exit 1
}
# Validate both ESP bundles before declaring MSHV in unsigned COSI metadata.
if [[ $variant == mshv ]]; then
    esp=$(jq -r '.images[] | select(.mountPoint == "/boot") | .image.path' "$scratch/metadata.json")
    [[ $esp =~ ^images/[a-zA-Z0-9._-]+$ ]]
    tar -xOf "$update" "$esp" > "$scratch/esp.zst"
    expected=$(jq -r '.images[] | select(.mountPoint == "/boot") | .image.sha384' "$scratch/metadata.json")
    printf '%s  %s\n' "$expected" "$scratch/esp.zst" | sha384sum --check -
    zstd -dq "$scratch/esp.zst" -o "$scratch/esp.raw"
    for image in "$base" "$scratch/esp.raw"; do
        device=/dev/sda1
        [[ $image == "$base" ]] || device=/dev/sda
        {
            printf 'run\nmount-ro %s /\n' "$device"
            for name in bootx64.efi grubx64.efi second.efi HvLoader.efi lxhvloader.dll \
                Windows/System32/hvix64.exe Windows/System32/hvax64.exe Windows/System32/kdstub.dll; do
                printf 'checksum sha256 /EFI/BOOT/%s\n' "$name"
            done
        } | guestfish --ro -a "$image" > "$scratch/$(basename "$image").boot"
    done
    cmp "$scratch/$(basename "$base").boot" "$scratch/esp.raw.boot"
    if grep -Fxq e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 "$scratch/esp.raw.boot"; then
        echo 'MSHV bundle contains an empty file' >&2
        exit 1
    fi
fi

cp --reflink=auto -- "$base" "$output/base.qcow2"
install -m 0600 "$key" "$output/id_rsa"
# IC's ACL detector hardcodes systemd-boot. Preserve all compressed partition
# bytes while correcting only the MSHV metadata tag in these unsigned fixtures.
tar -xOf "$update" cosi-marker > "$scratch/cosi-marker"
tar --delete -f - cosi-marker metadata.json < "$update" > "$scratch/payload.tar"
tar -cf "$output/update.cosi" -C "$scratch" cosi-marker metadata.json
tar --concatenate --file "$output/update.cosi" "$scratch/payload.tar"
cp -- "$scratch/metadata.json" "$output/metadata.json"
metadata_hash=$(sha384sum "$output/metadata.json" | cut -d ' ' -f 1)
base_hash=$(jq -r '.sha256' "$scratch/source.json")
for rpm in "$root"/bin/RPMS/*.rpm "$root"/artifacts/rpm-overrides/*.rpm; do
    [[ -f $rpm ]] || continue
    digest=$(sha256sum "$rpm" | cut -d ' ' -f 1)
    jq -cn --arg name "$(basename "$rpm")" --arg sha256 "$digest" '{name:$name, sha256:$sha256}'
done > "$scratch/rpms.jsonl"
jq -n --arg variant "$variant" --arg base "$base_hash" --arg binary "$base_binary" \
    --arg metadata "$metadata_hash" --slurpfile rpms "$scratch/rpms.jsonl" \
    --slurpfile source "$scratch/source.json" \
    '{imageSource:"acl-t", variant:$variant, baseImageSha256:$base,
      expectedTridentSha256:$binary, metadataSha384:$metadata, rpms:$rpms,
      aclTemplateRun:$source[0].runId, aclTemplateSource:$source[0], efitreeloaderRun:1261060,
      imageCustomizerRevision:"c8dc566c0c04d33e2deaa6a484113e0545997f92"}' > "$output/manifest.json"
