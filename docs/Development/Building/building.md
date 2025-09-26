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

## Building OS Modifier

Trident depends on `osmodifier`, a tool coming from the
[Azure Linux Image Tools](https://github.com/microsoft/azure-linux-image-tools)
repository, to perform OS image modifications.

To build `osmodifier` locally, run:

```bash
# Clone the Azure Linux Image Tools repository 
git clone https://github.com/microsoft/azure-linux-image-tools.git

# Run the osmodifier make target
make -C ./azure-linux-image-tools/toolkit go-osmodifier

# Copy the built binary to the artifacts directory
cp ./azure-linux-image-tools/toolkit/out/tools/osmodifier artifacts/osmodifier

# Clean up
rm -rf ./azure-linux-image-tools
```

Alternatively, you can build `osmodifier` on Azure Linux 3 using Docker:

```bash
# Clone the Azure Linux Image Tools repository 
git clone https://github.com/microsoft/azure-linux-image-tools.git

# Build osmodifier inside an Azure Linux 3 container
docker build -t trident/osmodifier-build:latest \
    -f Dockerfile-osmodifier.azl3 \
    .
mkdir -p artifacts
ID=$(docker create trident/osmodifier-build:latest)
docker cp -q $ID:/work/azure-linux-image-tools/toolkit/out/tools/osmodifier artifacts/osmodifier
docker rm -v $ID
```

:::info

`osmodifier` is a portable Golang binary, so you can build it on any Linux
distribution and use it on Azure Linux 3, but it is still recommended to build it
on the same OS to avoid any potential compatibility issues.

:::

## Building RPMs for Azure Linux

```bash
docker build -t trident/trident-build:latest \
    -f packaging/docker/Dockerfile.full.public \
    .
ID=$(docker create trident/trident-build:latest)
docker cp -q $ID:/work/trident-rpms.tar.gz trident-rpms.tar.gz
docker rm -v $ID
```
