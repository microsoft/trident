#!/usr/bin/env bash

set -euxo pipefail

readonly RUST_TOOLCHAIN="1.93.0"
readonly PROTOC_VERSION="33.2"
readonly PROTOC_SHA256="b24b53f87c151bfd48b112fe4c3a6e6574e5198874f38036aff41df3456b8caf"
readonly PROTOC_GEN_GO_VERSION="v1.36.11"
readonly PROTOC_GEN_GO_GRPC_VERSION="v1.6.2"
readonly DATA_DISK_MOUNT="/mnt/storage"
readonly CARGO_TARGET_PATH="$DATA_DISK_MOUNT/trident-cloud-agent/cargo-target"

sudo mkdir -p /etc/apt/apt.conf.d
echo 'DPkg::Lock::Timeout "600";' | sudo tee /etc/apt/apt.conf.d/99lock-timeout

sudo apt-get update
sudo apt-get install -y --no-install-recommends \
    build-essential \
    ca-certificates \
    clang \
    curl \
    libclang-dev \
    libssl-dev \
    pkg-config \
    unzip

protoc_archive="protoc-${PROTOC_VERSION}-linux-x86_64.zip"
curl --fail --location --retry 3 \
    --output "$RUNNER_TEMP/$protoc_archive" \
    "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/${protoc_archive}"
echo "$PROTOC_SHA256  $RUNNER_TEMP/$protoc_archive" | sha256sum --check
sudo unzip -o "$RUNNER_TEMP/$protoc_archive" -d /usr/local
rm -f "$RUNNER_TEMP/$protoc_archive"

go_bin_dir="$(go env GOPATH)/bin"
export PATH="$go_bin_dir:$PATH"
echo "$go_bin_dir" >> "$GITHUB_PATH"
go install "google.golang.org/protobuf/cmd/protoc-gen-go@${PROTOC_GEN_GO_VERSION}"
go install "google.golang.org/grpc/cmd/protoc-gen-go-grpc@${PROTOC_GEN_GO_GRPC_VERSION}"

if ! command -v rustup >/dev/null 2>&1; then
    echo "The Trident runner image must provide rustup" >&2
    exit 1
fi

export PATH="$HOME/.cargo/bin:$PATH"
echo "$HOME/.cargo/bin" >> "$GITHUB_PATH"

rustup toolchain install "$RUST_TOOLCHAIN" \
    --profile minimal \
    --component clippy,rustfmt
rustup default "$RUST_TOOLCHAIN"

if ! mountpoint --quiet "$DATA_DISK_MOUNT"; then
    echo "Expected runner data disk is not mounted at $DATA_DISK_MOUNT" >&2
    exit 1
fi
findmnt --mountpoint "$DATA_DISK_MOUNT"

sudo install -d -o "$(id -u)" -g "$(id -g)" "$CARGO_TARGET_PATH"

make .cargo/config
if [[ -e target && ! -L target ]]; then
    if [[ ! -d target || -n "$(find target -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
        echo "Refusing to replace existing non-empty target path" >&2
        exit 1
    fi
    rmdir target
fi
ln -sfn "$CARGO_TARGET_PATH" target
test "$(readlink -f target)" = "$CARGO_TARGET_PATH"

cargo fetch --locked

rustc --version
cargo --version
go version
protoc --version
protoc-gen-go --version
protoc-gen-go-grpc --version
