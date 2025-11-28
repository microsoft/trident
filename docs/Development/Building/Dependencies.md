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
  sudo apt install build-essential pkg-config libssl-dev libclang-dev protobuf-compiler
  ```

- Docker (follow the instructions at [Install Docker Engine on Ubuntu](https://docs.docker.com/engine/install/ubuntu/))

## Test Dependencies

- Python 3.8+
- Python packages:

  ```bash
  sudo pip3 install virt-firmware
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

