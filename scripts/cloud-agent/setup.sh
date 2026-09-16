#!/usr/bin/env bash

set -euxo pipefail

readonly RUST_TOOLCHAIN="1.98.1"
readonly PROTOC_GEN_GO_VERSION="v1.36.11"
readonly PROTOC_GEN_GO_GRPC_VERSION="v1.6.2"
readonly DATA_DISK_MOUNT="/mnt/storage"
readonly CARGO_TARGET_PATH="$DATA_DISK_MOUNT/trident-cloud-agent/cargo-target"
readonly CARGO_CREDENTIAL_PROVIDER_PATH="$HOME/.local/bin/trident-ado-cargo-token"

: "${ADO_UMI_CLIENT_ID:?ADO_UMI_CLIENT_ID must identify the runner managed identity}"

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

make install-protoc

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

az login \
    --identity \
    --client-id "$ADO_UMI_CLIENT_ID" \
    --allow-no-subscriptions \
    >/dev/null

install -D -m 0755 \
    scripts/cloud-agent/ado-cargo-token.sh \
    "$CARGO_CREDENTIAL_PROVIDER_PATH"

make -B OVERRIDE_RUST_FEED=false .cargo/config
grep -Eq '^[[:space:]]*replace-with[[:space:]]*=[[:space:]]*"BMP_PublicPackages"' \
    .cargo/config
printf '\n[registry]\nglobal-credential-providers = ["cargo:token-from-stdout %s"]\n' \
    "$CARGO_CREDENTIAL_PROVIDER_PATH" \
    >> .cargo/config
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
