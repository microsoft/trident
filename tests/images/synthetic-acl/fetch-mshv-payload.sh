#!/bin/bash
set -euo pipefail
shopt -s nullglob dotglob

if [[ $# != 2 || $1 != --output-dir || -z $2 ]]; then
    printf 'Usage: %s --output-dir PATH\n' "$0" >&2
    exit 1
fi
for tool in curl jq sha256sum rpm2cpio cpio; do
    command -v "$tool" > /dev/null || { printf 'Missing required tool: %s\n' "$tool" >&2; exit 1; }
done
output=$(realpath -m -- "$2")
mkdir -p -- "$output"
if [[ -n $(find "$output" -mindepth 1 -maxdepth 1 -print -quit) ]]; then
    printf 'Payload output directory must be empty\n' >&2
    exit 1
fi
rpms="$output/.rpms"
extraction="$output/.extracted"
mkdir "$rpms" "$extraction"
lock="$(dirname -- "${BASH_SOURCE[0]}")/mshv-packages.json"
jq -er '.packages[] | [.name, .url, .sha256] | @tsv' "$lock" > "$output/packages.tsv"
while IFS=$'\t' read -r name url digest; do
    rpm="$rpms/${url##*/}"
    curl --fail --location --retry 3 --output "$rpm" "$url"
    if ! printf '%s  %s\n' "$digest" "$rpm" | sha256sum --check --status; then
        printf 'RPM SHA-256 mismatch: %s\n' "$name" >&2
        exit 1
    fi
    rpm2cpio "$rpm" | (
        cd "$extraction"
        cpio --extract --make-directories --no-absolute-filenames --quiet \
            './boot/efi/*' 'boot/efi/*' './usr/share/licenses/*' 'usr/share/licenses/*'
    )
done < "$output/packages.tsv"
files=("$extraction/boot/efi/"*)
((${#files[@]} > 0)) || { printf 'Missing extracted EFI payload\n' >&2; exit 1; }
mv -- "${files[@]}" "$output/"
mv -- "$extraction/usr/share/licenses" "$output/licenses"
rmdir "$extraction/boot/efi" "$extraction/boot" "$extraction/usr/share" "$extraction/usr" "$extraction"
cp -- "$lock" "$output/payload-provenance.json"
rm -- "$output/packages.tsv"
