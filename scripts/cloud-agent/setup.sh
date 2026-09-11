#!/usr/bin/env bash

set -euxo pipefail

readonly RUST_TOOLCHAIN="1.93.0"
readonly PROTOC_VERSION="33.2"
readonly PROTOC_SHA256="b24b53f87c151bfd48b112fe4c3a6e6574e5198874f38036aff41df3456b8caf"
readonly CARGO_TARGET_DIR="/mnt/trident-cloud-agent/cargo-target"

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

if ! command -v rustup >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 --fail --silent --show-error \
        https://sh.rustup.rs |
        sh -s -- -y --profile minimal --default-toolchain none
fi

export PATH="$HOME/.cargo/bin:$PATH"
echo "$HOME/.cargo/bin" >> "$GITHUB_PATH"

rustup toolchain install "$RUST_TOOLCHAIN" \
    --profile minimal \
    --component clippy,rustfmt
rustup default "$RUST_TOOLCHAIN"

sudo install -d -o "$(id -u)" -g "$(id -g)" "$CARGO_TARGET_DIR"

make .cargo/config
if ! grep -q '^\[build\]$' .cargo/config; then
    cat >> .cargo/config <<EOF

[build]
target-dir = "$CARGO_TARGET_DIR"
EOF
fi

cargo fetch --locked

rustc --version
cargo --version
go version
protoc --version
