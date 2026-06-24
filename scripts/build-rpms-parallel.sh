#!/bin/bash

# Builds the azl3 and azl4 Trident RPMs concurrently against a single
# BuildKit daemon, then unpacks each into the artifact layout (azl3 at the
# base directory, azl4 under azl4/). Running both docker builds as background
# jobs in one task avoids a second ADO job and the duplicated OneBranch/setup
# overhead while overlapping the I/O-bound parts of the two builds.
#
# Usage:
#   build-rpms-parallel.sh <full_version> <dockerfile> <artifact_dir> <work_dir>
#
# Args:
#   full_version  Full Trident version string (e.g. from get-version.py).
#   dockerfile    Path to Dockerfile.full.
#   artifact_dir  Base artifact directory (azl3 lands here, azl4 under azl4/).
#   work_dir      Scratch directory for per-distro build output and logs.
#                 Should be job-scoped (e.g. $(Agent.TempDirectory)/...).
#
# Expects CARGO_REGISTRIES_BMP_PUBLICPACKAGES_TOKEN in the environment
# (populated by the CargoAuthenticate task) for the docker build secret.

set -euxo pipefail

full_version=$1
dockerfile=$2
artifact_dir=$3
work_dir=$4

# Separate into version and prerelease identifier for the RPM build.
version=$(echo "$full_version" | cut -d'-' -f1)
prerelease=$(echo "$full_version" | cut -d'-' -f2-)

build_one() {
  # args: <distro> <dest_dir> <log_file>
  local distro="$1"
  local dest="$2"
  local log="$3"

  # Resolve all distro vars from a single make invocation so a make failure
  # isn't masked by the grep/cut pipeline (pipefail also guards).
  local vars azl_image distro_packages rpm_packages rpm_dest rust_package
  vars="$(DISTRO=$distro make azl-version-vars)"
  azl_image="$(echo "$vars" | grep '^AZL_IMAGE=' | cut -d '=' -f2)"
  distro_packages="$(echo "$vars" | grep '^DISTRO_PACKAGES=' | cut -d '=' -f2)"
  rpm_packages="$(echo "$vars" | grep '^RPM_PACKAGES=' | cut -d '=' -f2)"
  rpm_dest="$(echo "$vars" | grep '^RPM_DEST=' | cut -d '=' -f2)"
  rust_package="$(echo "$vars" | grep '^RUST_PACKAGE=' | cut -d '=' -f2)"

  rm -rf "$dest"
  mkdir -p "$dest"

  # Per-distro image tag avoids collisions between the two concurrent builds.
  docker build -f "$dockerfile" -t "trident/trident-build:$distro" \
    --secret id=registry_token,env=CARGO_REGISTRIES_BMP_PUBLICPACKAGES_TOKEN \
    --build-arg TRIDENT_VERSION="$full_version" \
    --build-arg RPM_VER="$version" \
    --build-arg RPM_REL="$prerelease.$distro" \
    --build-arg AZL_IMAGE="$azl_image" \
    --build-arg DISTRO_PACKAGES="$distro_packages" \
    --build-arg RPM_PACKAGES="$rpm_packages" \
    --build-arg RUST_PACKAGE="$rust_package" \
    --build-arg RPM_DEST="$rpm_dest" \
    --target artifact \
    --output type=local,dest="$dest" \
    . > "$log" 2>&1
}

rm -rf "$work_dir"
mkdir -p "$work_dir"

# Launch both builds in the background.
build_one azl3 "$work_dir/azl3" "$work_dir/azl3.log" &
pid_azl3=$!
build_one azl4 "$work_dir/azl4" "$work_dir/azl4.log" &
pid_azl4=$!

# Wait for each and capture exit codes (don't let set -e abort early).
rc_azl3=0
rc_azl4=0
wait "$pid_azl3" || rc_azl3=$?
wait "$pid_azl4" || rc_azl4=$?

# Surface both logs sequentially so concurrent output is readable.
echo "===== azl3 build log ====="
cat "$work_dir/azl3.log" || true
echo "===== azl4 build log ====="
cat "$work_dir/azl4.log" || true

if [ "$rc_azl3" -ne 0 ] || [ "$rc_azl4" -ne 0 ]; then
  echo "Build failed: azl3 rc=$rc_azl3, azl4 rc=$rc_azl4"
  exit 1
fi

# Unpack azl3 to the base artifact dir, azl4 under azl4/.
mkdir -p "$artifact_dir"
mkdir -p "$artifact_dir/azl4"
tar -xzf "$work_dir/azl3/trident-rpms.tar.gz" -C "$artifact_dir" --strip-components=3
tar -xzf "$work_dir/azl4/trident-rpms.tar.gz" -C "$artifact_dir/azl4" --strip-components=3
