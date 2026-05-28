---
sidebar_position: 2
---

# Building Trident

By default, this repo is configured to pull crates from an internal Microsoft
mirror which blocks known vulnerable crates and versions.

To build this using public mirrors, run
this to use public mirrors. It will create a new file `.cargo/config` that
removes the private mirror configuration.

```bash
make .cargo/config
```

:::note

Cargo only expects one config file, so this will cause cargo to complain about
finding two. This is expected. The new file will take precedence.

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

## Building RPMs for Azure Linux

```bash
docker build -t trident/trident-build:latest \
    -f packaging/docker/Dockerfile.full.public \
    .
ID=$(docker create trident/trident-build:latest)
docker cp -q $ID:/work/trident-rpms.tar.gz trident-rpms.tar.gz
docker rm -v $ID
```
