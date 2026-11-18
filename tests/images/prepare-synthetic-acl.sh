#!/bin/bash
set -euo pipefail
export LC_ALL=C
shopt -s nullglob

HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/synthetic-acl" && pwd)

fail() {
    printf '%s\n' "$*" >&2
    exit 1
}

sha256() {
    sha256sum < "$1" | cut -d ' ' -f 1
}

provenance() {
    local digest
    digest=$(sha256 "$1") || return
    jq -cn --arg path "$1" --arg sha256 "$digest" \
        '{path: $path, sha256: $sha256}'
}

fish_quote() {
    local value=$1
    [[ $value != *[$'\r\n']* ]] || fail 'Guestfish paths cannot contain line breaks'
    value=${value//\\/\\\\}
    value=${value//\"/\\\"}
    printf '"%s"' "$value"
}

fish() {
    local mode=$1 image=$2
    { printf 'run\n'; cat; } | guestfish "$mode" -a "$image"
}

pe_cmdline() {
    local image=$1
    objdump -h "$image" > "$image.sections"
    if awk '$2 == ".cmdline" { found = 1 } END { exit !found }' "$image.sections"; then
        objcopy --dump-section ".cmdline=$image.cmdline" "$image"
        tr -d '\000' < "$image.cmdline"
    fi
}

verity_cmdlines() {
    local cmdline=$1 directory=$2 token root_hash='' slot data hashes
    local -a tokens common=()
    read -r -a tokens <<< "$cmdline"
    for token in "${tokens[@]}"; do
        case "$token" in
            usrhash=*)
                [[ -z $root_hash || $root_hash == "${token#*=}" ]] ||
                    fail 'Conflicting verity root hashes'
                root_hash=${token#*=}
                ;;
            acl.slot=*|systemd.verity_usr_data=*|systemd.verity_usr_hash=*|systemd.verity_usr_options=*) ;;
            root=*) common+=('root=LABEL=ROOT') ;;
            *) common+=("$token") ;;
        esac
    done
    [[ $root_hash =~ ^([[:xdigit:]]{64}|[[:xdigit:]]{128})$ ]] ||
        fail 'Expected one valid verity root hash in the ImageCustomizer command line'
    printf '%s\0' "${common[*]}" > "$directory/common.cmdline"
    for slot in a b; do
        if [[ $slot == a ]]; then
            data=7130c94a-213a-4e5a-8e26-6cce9662f132
            hashes=b736baf1-cdb4-4535-beba-ddaaa30ad7b7
        else
            data=e03dd35c-7c2d-4a47-b3fe-27f15780a57c
            hashes=35bdf78b-c453-4661-98e6-f834f534ef5b
        fi
        printf 'systemd.verity_usr_data=PARTUUID=%s systemd.verity_usr_hash=PARTUUID=%s systemd.verity_usr_options=panic-on-corruption usrhash=%s acl.slot=%s\n' \
            "$data" "$hashes" "$root_hash" "$slot" > "$directory/slot-$slot.cmdline"
    done
}

normalize_disk() {
    local image=$1 name
    {
        cat <<'LAYOUT'
part-set-gpt-guid /dev/sda 2 7130c94a-213a-4e5a-8e26-6cce9662f132
part-set-gpt-guid /dev/sda 3 b736baf1-cdb4-4535-beba-ddaaa30ad7b7
part-set-gpt-guid /dev/sda 4 e03dd35c-7c2d-4a47-b3fe-27f15780a57c
part-set-gpt-guid /dev/sda 5 35bdf78b-c453-4661-98e6-f834f534ef5b
part-set-gpt-type /dev/sda 2 5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6
part-set-gpt-type /dev/sda 4 5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6
set-label /dev/sda1 EFI-SYSTEM
set-label /dev/sda7 ROOT
mount /dev/sda7 /
mount /dev/sda1 /boot/efi
LAYOUT
        printf 'upload %s /etc/fstab\n' "$(fish_quote "$HERE/fstab")"
        if [[ $variant == mshv ]]; then
            # shim -> ETL -> HvLoader -> second.efi (systemd-boot) -> UKI.
            printf 'cp /boot/efi/EFI/BOOT/grubx64.efi /boot/efi/EFI/BOOT/second.efi\n'
            printf 'upload %s /boot/efi/EFI/BOOT/grubx64.efi\n' "$(fish_quote "$efitreeloader")"
            for name in HvLoader.efi lxhvloader.dll; do
                printf 'upload %s /boot/efi/EFI/BOOT/%s\n' "$(fish_quote "$mshv_payload/$name")" "$name"
            done
            printf 'copy-in %s /boot/efi/EFI/BOOT\n' "$(fish_quote "$mshv_payload/Windows")"
        fi
        # Remove the builder mountpoint from shared ROOT, not from the ESP.
        printf 'umount /boot/efi\nrmdir /boot/efi\numount-all\n'
    } | fish --rw "$image"
}

install_verity_addons() {
    local image=$1 directory=$2 uki provider='' cmdline='' path value slot addon relative
    local -a ukis
    mkdir "$directory"
    fish --ro "$image" <<EOF
mount-ro /dev/sda2 /
download /lib/systemd/boot/efi/linuxx64.efi.stub $(fish_quote "$directory/stub.efi")
download /lib/os-release $(fish_quote "$directory/os-release")
umount-all
mount-ro /dev/sda1 /
copy-out /EFI/Linux $(fish_quote "$directory")
umount-all
EOF
    ukis=("$directory/Linux/"*.efi)
    [[ ${#ukis[@]} == 1 ]] || fail 'Expected exactly one synthetic image UKI'
    uki=${ukis[0]}
    for path in "$uki" "$uki.extra.d/"*.addon.efi; do
        value=$(pe_cmdline "$path")
        if [[ -n $value ]]; then
            [[ -z $provider ]] || fail 'Expected exactly one ImageCustomizer command-line section'
            provider=$path
            cmdline=$value
        fi
    done
    [[ -n $provider ]] || fail 'Missing ImageCustomizer command-line section'
    verity_cmdlines "$cmdline" "$directory"
    objcopy --update-section ".cmdline=$directory/common.cmdline" "$provider" "$provider.rewritten"
    mv -- "$provider.rewritten" "$provider"
    [[ $(pe_cmdline "$provider") == "$(tr -d '\000' < "$directory/common.cmdline")" ]] ||
        fail 'Rewritten UKI command line does not match the slot-independent policy'
    relative=${provider#"$directory/"}
    {
        printf 'mount /dev/sda1 /\nupload %s /EFI/%s\n' "$(fish_quote "$provider")" "$relative"
        printf 'mkdir-p /acl/uki-addons\nmkdir-p /EFI/Linux/%s.extra.d\n' "${uki##*/}"
        for slot in a b; do
            addon="$directory/slot-$slot.addon.efi"
            docker run --rm --entrypoint /usr/bin/ukify -e TMPDIR=/work \
                -v "$directory:/work" "$image_id" build --stub=/work/stub.efi \
                --os-release=@/work/os-release "--cmdline=@/work/slot-$slot.cmdline" \
                "--output=/work/slot-$slot.addon.efi" >&2
            [[ $(pe_cmdline "$addon") == "$(cat "$directory/slot-$slot.cmdline")" ]] ||
                fail "Slot $slot addon does not contain its requested verity command line"
            printf 'upload %s /acl/uki-addons/slot-%s.addon.efi\n' "$(fish_quote "$addon")" "$slot"
            if [[ $slot == a ]]; then
                printf 'upload %s /EFI/Linux/%s.extra.d/slot-a.addon.efi\n' "$(fish_quote "$addon")" "${uki##*/}"
            fi
        done
        printf 'umount-all\n'
    } | fish --rw "$image"
}

publish_metadata() {
    local cosi="$output/update.cosi" directory="$work/cosi-metadata"
    mkdir "$directory"
    tar -xOf "$cosi" metadata.json > "$directory/original.json" || return
    jq -e --arg variant "$variant" '
        if any(.images[]; .mountPoint == "/usr" and (.verity.roothash | type == "string" and length > 0))
            and (.bootloader.systemdBoot.entries | type == "array" and length > 0)
            and (.bootloader.type == "systemd-boot"
                or ($variant == "mshv" and .bootloader.type == "efitreeloader-mshv-systemd-boot"))
        then if $variant == "mshv"
            then .version = "1.3" | .bootloader.type = "efitreeloader-mshv-systemd-boot"
            else . end
        else error("COSI must contain a verity-protected /usr and systemd-boot UKI metadata")
        end' "$directory/original.json" > "$directory/metadata.json" || return
    tar -xOf "$cosi" cosi-marker > "$directory/cosi-marker" || return
    # Preserve the COSI marker/metadata prefix and compressed partition bytes.
    tar --delete -f - cosi-marker metadata.json < "$cosi" > "$work/cosi-payload.tar"
    tar -cf "$work/update.rewritten.cosi" -C "$directory" cosi-marker metadata.json
    tar --concatenate --file "$work/update.rewritten.cosi" "$work/cosi-payload.tar"
    mv -- "$work/update.rewritten.cosi" "$cosi"
    cp -- "$directory/metadata.json" "$output/metadata.json"
    rm -- "$work/cosi-payload.tar"
}

main() {
    local name fields rpm package candidate=0 version='' release='' rpm_version rpm_release rpm_arch
    local public expected phase binary_hash update_hash loader_hash='' digest base_provenance metadata_hash
    local -a rpms removals=()
    variant='' base_image='' rpm_dir='' ssh_public_key='' ssh_private_key='' output_dir='' image_customizer=''
    efitreeloader='' mshv_payload='' loader_provenance=''
    while (($#)); do
        if [[ $1 == --help ]]; then
            printf 'Usage: %s --variant standard|mshv --base-image PATH --rpm-dir PATH --ssh-public-key PATH --ssh-private-key PATH --output-dir PATH --image-customizer IMAGE [--efitreeloader PATH --mshv-payload PATH --loader-provenance TEXT]\n' "$0"
            return
        fi
        (($# >= 2)) || fail "Missing value for $1"
        case "$1" in
            --variant|--base-image|--rpm-dir|--ssh-public-key|--ssh-private-key|--output-dir|--image-customizer|--efitreeloader|--mshv-payload|--loader-provenance)
                name=${1#--}
                name=${name//-/_}
                printf -v "$name" '%s' "$2"
                ;;
            *) fail "Unknown option: $1" ;;
        esac
        shift 2
    done
    for name in variant base_image rpm_dir ssh_public_key ssh_private_key output_dir image_customizer; do
        [[ -n ${!name} ]] || fail "Missing required option: --${name//_/-}"
    done
    [[ $variant == standard || $variant == mshv ]] || fail 'Variant must be standard or mshv'
    ((EUID == 0)) || fail 'Run with sudo: ImageCustomizer creates root-owned disk images'
    for name in docker guestfish rpm ssh-keygen objcopy objdump jq tar sha256sum sha384sum; do
        command -v "$name" > /dev/null || fail "Missing required tool: $name"
    done
    for name in base_image rpm_dir ssh_public_key ssh_private_key; do
        printf -v "$name" '%s' "$(realpath -e -- "${!name}")"
    done
    if [[ $variant == mshv ]]; then
        [[ -n $efitreeloader && -n $mshv_payload && -n $loader_provenance ]] ||
            fail 'mshv requires --efitreeloader, --mshv-payload and --loader-provenance'
        efitreeloader=$(realpath -e -- "$efitreeloader")
        mshv_payload=$(realpath -e -- "$mshv_payload")
        [[ -f $mshv_payload/HvLoader.efi && -f $mshv_payload/lxhvloader.dll ]] ||
            fail 'Missing MSHV HvLoader.efi or lxhvloader.dll'
        [[ -d $mshv_payload/Windows && -n $(find "$mshv_payload/Windows" -type f -print -quit) ]] ||
            fail 'Missing nonempty MSHV Windows component tree'
        [[ -z $(find "$mshv_payload" -mindepth 1 ! -type f ! -type d -print -quit) ]] ||
            fail 'Payload must contain only regular files/directories'
    elif [[ -n $efitreeloader || -n $mshv_payload || -n $loader_provenance ]]; then
        fail 'standard must not contain MSHV loader inputs'
    fi
    public=$(ssh-keygen -y -P '' -f "$ssh_private_key" | awk 'NF { print $1, $2; exit }')
    expected=$(awk 'NF { print $1, $2; exit }' "$ssh_public_key")
    [[ $public == "$expected" ]] || fail 'SSH public/private keys do not match'
    rpms=("$rpm_dir/"*.rpm)
    for rpm in "${rpms[@]}"; do
        fields=$(rpm -qp --qf '%{NAME} %{VERSION} %{RELEASE} %{ARCH}' "$rpm")
        read -r package rpm_version rpm_release rpm_arch <<< "$fields"
        if [[ $package == trident && $rpm_arch == x86_64 ]]; then
            version=$rpm_version
            release=$rpm_release
            candidate=$((candidate + 1))
        fi
    done
    [[ $candidate == 1 ]] || fail 'RPM directory must contain exactly one x86_64 trident RPM'
    output=$(realpath -m -- "$output_dir")
    mkdir -p -- "$output"
    [[ -z $(find "$output" -mindepth 1 -maxdepth 1 -print -quit) ]] ||
        fail 'Output directory must be empty (failed builds retain diagnostics)'
    work="$output/.work"
    mkdir "$work"
    export TMPDIR="$work/tmpdir" LIBGUESTFS_TMPDIR="$work/libguestfs_tmpdir" LIBGUESTFS_CACHEDIR="$work/libguestfs_cachedir"
    mkdir "$TMPDIR" "$LIBGUESTFS_TMPDIR" "$LIBGUESTFS_CACHEDIR"
    guestfish --ro -a "$base_image" -i cat /etc/os-release > "$work/base-os-release"
    [[ $(awk -F= '$1 == "ID" { gsub(/"/, "", $2); print $2 }' "$work/base-os-release") == azurelinux &&
        $(awk -F= '$1 == "VERSION_ID" { gsub(/"/, "", $2); print $2 }' "$work/base-os-release") == 3.0 &&
        $(awk -F= '$1 == "VARIANT_ID" { gsub(/"/, "", $2); print $2 }' "$work/base-os-release") != azurecontainerlinux ]] ||
        fail 'Base image must be plain x86-64 Azure Linux 3.0, not an ACL/nightly image'
    # Query the guest: older libguestfs cannot inspect AZL's SQLite RPM database.
    guestfish --ro -a "$base_image" -i command 'rpm -qa --qf %{NAME}|' |
        tr '|' '\n' > "$work/installed-packages"
    grep -q '[[:alnum:]]' "$work/installed-packages" || fail 'Could not read the base RPM inventory'
    for name in grub2-efi-binary kernel kernel-mshv shim; do
        if [[ ($variant == mshv && $name != kernel-mshv) ||
            ($variant == standard && ($name == grub2-efi-binary || $name == kernel-mshv)) ]]; then
            if grep -Fxq "$name" "$work/installed-packages"; then removals+=("$name"); fi
        fi
    done
    jq -n --arg nevra "trident-$version-$release.x86_64" '{packages: [$nevra]}' > "$work/trident-packages.json"
    printf '%s\n' "${removals[@]}" | jq -Rs '{packages: split("\n") | map(select(length > 0))}' > "$work/remove-packages.json"
    install -m 0600 "$ssh_private_key" "$output/id_rsa"
    cp -p -- "$ssh_public_key" "$work/id_rsa.pub"
    mkdir "$work/rpms"
    cp -p -- "${rpms[@]}" "$work/rpms/"
    cp -p -- "$HERE/$variant-base.yaml" "$HERE/$variant-update.yaml" "$HERE/configure.sh" "$HERE/99-synthetic-acl.conf" "$work/"
    if [[ $variant == mshv ]]; then
        if [[ -d $mshv_payload/licenses ]]; then cp -a -- "$mshv_payload/licenses" "$work/mshv-licenses"
        else mkdir "$work/mshv-licenses"; fi
    fi
    image_id=$(docker image inspect --format '{{.Id}}' "$image_customizer")
    docker run --rm --entrypoint /usr/bin/ukify "$image_id" --version
    for phase in base update; do
        docker run --rm --privileged -v "$work:/work" -v "$base_image:/input/base.img:ro" \
            -v /dev:/dev "$image_id" customize --config-file "/work/$variant-$phase.yaml" \
            --build-dir "/work/build-$phase" --image-file /input/base.img \
            --output-image-format qcow2 --output-image-file "/work/$phase.qcow2" --rpm-source /work/rpms
        normalize_disk "$work/$phase.qcow2"
        install_verity_addons "$work/$phase.qcow2" "$work/uki-$phase"
    done
    mv -- "$work/base.qcow2" "$output/base.qcow2"
    docker run --rm --privileged -v "$work:/work" -v "$output:/output" -v /dev:/dev \
        "$image_id" convert --build-dir /work/build-cosi --image-file /work/update.qcow2 \
        --output-image-format cosi --output-image-file /output/update.cosi
    publish_metadata
    binary_hash=$(fish --ro "$output/base.qcow2" <<'FISH'
mount-ro /dev/sda2 /
checksum sha256 /bin/trident
umount-all
FISH
)
    update_hash=$(fish --ro "$work/update.qcow2" <<'FISH'
mount-ro /dev/sda2 /
checksum sha256 /bin/trident
umount-all
FISH
)
    [[ $binary_hash =~ ^[[:xdigit:]]{64}$ && $binary_hash == "$update_hash" ]] ||
        fail 'Base and update must contain identical pipeline Trident binaries'
    for rpm in "${rpms[@]}"; do provenance "$rpm"; done > "$work/rpm-provenance.jsonl"
    : > "$work/payload-provenance.jsonl"
    if [[ $variant == mshv ]]; then
        loader_hash=$(sha256 "$efitreeloader")
        while IFS= read -r -d '' name; do provenance "$name"; done \
            < <(find "$mshv_payload" -type f -print0 | sort -z) > "$work/payload-provenance.jsonl"
    fi
    for name in base.qcow2 update.cosi metadata.json; do
        digest=$(sha256 "$output/$name")
        jq -cn --arg key "$name" --arg sha256 "$digest" '{key: $key, value: {sha256: $sha256}}'
    done > "$work/output-provenance.jsonl"
    base_provenance=$(provenance "$base_image")
    metadata_hash=$(sha384sum < "$output/metadata.json" | cut -d ' ' -f 1)
    jq -n --arg variant "$variant" --arg version "$version" --arg release "$release" \
        --arg binary "$binary_hash" --argjson base "$base_provenance" \
        --arg reference "$image_customizer" --arg imageId "$image_id" \
        --arg metadataSha384 "$metadata_hash" \
        --arg loader "$efitreeloader" --arg loaderHash "$loader_hash" --arg loaderProvenance "$loader_provenance" \
        --slurpfile rpms "$work/rpm-provenance.jsonl" --slurpfile payload "$work/payload-provenance.jsonl" \
        --slurpfile outputs "$work/output-provenance.jsonl" '
        {variant: $variant, synthetic: true, expectedTridentVersion: $version,
         expectedTridentRelease: $release, expectedTridentSha256: $binary,
         osRelease: {ID: "azurelinux", VARIANT_ID: "azurecontainerlinux", VERSION_ID: "3.0"},
         network: {dhcp: true, address: null},
         inputs: {baseImage: $base, rpms: $rpms, imageCustomizer: {reference: $reference, imageId: $imageId}},
         metadataSha384: $metadataSha384, outputs: ($outputs | from_entries)}
        | if $variant == "mshv" then .inputs += {
            efitreeloader: {path: $loader, sha256: $loaderHash, provenance: $loaderProvenance},
            mshvPayload: $payload} else . end' > "$output/manifest.json"
}

if [[ ${BASH_SOURCE[0]} == "$0" ]]; then
    main "$@"
fi
