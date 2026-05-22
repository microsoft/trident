---
sidebar_position: 1
---

# Requirements

Trident's dependencies.

## Build Dependencies

### Compilers

- Rust: latest stable
- Go: 1.25+ (for osmodifier)

### Packages

- Ubuntu/Debian:

  ```bash
  sudo apt install build-essential pkg-config libssl-dev libclang-dev protobuf-compiler ca-certificates unzip
  ```

  :::warning protobuf-compiler version
  Building Go tools that use gRPC (e.g., `netlaunch`, `storm-trident`) requires
  `protoc` 3.15+ for proto3 optional field support. On Ubuntu 22.04 and earlier,
  the apt `protobuf-compiler` package is too old (3.12). Install a newer version
  manually:

  ```bash
  PROTOC_VERSION=28.3
  curl -sL https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-x86_64.zip -o /tmp/protoc.zip
  sudo unzip -o /tmp/protoc.zip -d /usr/local
  rm /tmp/protoc.zip
  ```

  You also need the Go protobuf plugins:

  ```bash
  go install google.golang.org/protobuf/cmd/protoc-gen-go@latest
  go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@latest
  ```
  :::

  For RPM builds (run inside the Azure Linux build container, not on the
  Ubuntu host), additional packages are needed: `rpmdevtools`, `sed`,
  `perl`, and `selinux-policy-devel`.

- Docker (follow the instructions at [Install Docker Engine on Ubuntu](https://docs.docker.com/engine/install/ubuntu/))

## Test Dependencies

- Python 3.8+
- Python packages:

  ```bash
  # Use version 26.2 to avoid a breaking change
  # introduced in 26.4.
  sudo pip3 install virt-firmware==26.2
  ```

## Code Coverage Dependencies

- `cargo-llvm-cov`

  ```bash
  cargo install cargo-llvm-cov --locked
  ```

- `cargo-nextest`

  ```bash
  cargo install cargo-nextest --locked
  ```

