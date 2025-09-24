---
sidebar_position: 2
---

# Building Trident

By default, this repo is configured to pull crates from an internal Microsoft mirror.

To build this using public mirrors, run
this to use public mirrors. It will create a new file `.cargo/config` that
removes the private mirror configuration.

```bash
make .cargo/config
```

:::note

Cargo only expects one config file, so this will cause cargo to complain about finding two. This is expected.

:::

## Building Binary

To build Trident as a binary, run:

```bash
cargo build --release
```

or

```bash
make build
```

The binary will be placed at `target/release/trident`.

## Building OS Modifier

Trident depends on `osmodifier`, a tool coming from the [Azure Linux Image
Tools](https://github.com/microsoft/azure-linux-image-tools) git submodule to
perform OS image modifications. To build `osmodifier`, run:

```bash
# Ensure the git submodule is initialized
git submodule update --init --recursive

# Build osmodifier
docker build -t trident/osmodifier-build:latest \
    -f Dockerfile-osmodifier.azl3 \
    .
mkdir -p artifacts
ID=$(docker create trident/osmodifier-build:latest)
docker cp -q $ID:/work/azure-linux-image-tools/toolkit/out/tools/osmodifier artifacts/osmodifier
docker rm -v $ID
```

## Building RPMs for Azure Linux

```bash
docker build -t trident/trident-build:latest \
    -f packaging/docker/Dockerfile.full.public \
    .
ID=$(docker create trident/trident-build:latest)
docker cp -q $ID:/work/trident-rpms.tar.gz trident-rpms.tar.gz
docker rm -v $ID
```
