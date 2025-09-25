
# Run Trident Inside a Container

This guide explains how to run Trident inside a container.

## Goals

By following this guide, you will:

- Build a container image containing Trident.
- Load the image into your local container runtime.
- Run Trident inside a container.
- Understand the purpose of each flag and mounted directory required for Trident
  to function correctly.

## Prerequisites

1. This guide uses `docker` for all code snippets. However, the commands can be
   adapted to other tools.

## Instructions

### Step 1: Build a Container Image

Build the Trident container image using:

```bash
make artifacts/test-image/trident-container.tar.gz
```

This Make target will build the Trident RPMs (`make bin/trident-rpms.tar.gz`)
and then use [Dockerfile.runtime](../../Dockerfile.runtime) to build the
container image with all the necessary dependencies. You will find a compressed
form of containerized Trident at
`artifacts/test-image/trident-container.tar.gz`.

#### Note

If you plan to run to Trident in the same environment in which you build the
Trident container image, you can instead use:

```bash
make docker-build
```

This command will create the container image and save it to your local Docker
image repository. If you use this command, you can skip Step 2.

### Step 2: Load the Image

Load the Trident container image - `trident-container.tar.gz`, which you created
in the previous step - into your local Docker image repository:

```bash
docker load --input artifacts/test-image/trident-container.tar.gz
```

If you have renamed or changed the location of your Trident container image,
make sure to change the file path after the `--input` flag in the provided code
sample above.

### Step 3: Run Trident

Run Trident:

```bash
docker run --name trident_container \
           --pull=never \
           --rm \
           --privileged \
           -v /path/to/your/host-config:/etc/trident \
           -v /etc/pki:/etc/pki:ro \
           -v /var/lib/trident:/var/lib/trident \
           -v /var/log:/var/log \
           -v /:/host \
           -v /dev:/dev \
           -v /run:/run \
           -v /sys:/sys \
           --pid host \
           --ipc host \
           trident/trident:latest <TRIDENT VERB> /etc/trident/hostconf.yaml --verbosity TRACE
```

Note: Ensure that you replace `/path/to/your/host-config` with the actual path
to your Host Configuration on your host machine.

Replace `<TRIDENT VERB>` with the desired verb. For a complete explanation of
the Trident CLI, please see the [Reference guide](../Reference/Trident-CLI.md).

## Explanation of Docker Command

### Key Flags

- `--privileged`: Trident requires access to devices on the host to perform
  operations such as partitioning disks and creating filesystems.
- `--pid host` and `--ipc host`: Allows the container to share the host's
  process and inter-process communication namespaces. This is necessary for
  Trident to interact with other system services.
- `--rm`: Automatically removes the container when it exits, which is useful for
  cleanup.
- `--pull=never`: Ensures the command uses the local `trident/trident:latest`
  image (built in Step 1) and does not try to download it from a remote
  registry.

### Mounted Volumes

- `-v /path/to/your/host-config:/etc/trident`: Trident expects to find the Host
Configuration and [Agent Configuration](../Reference/Agent-Configuration.md)
files in the `/etc/trident` directory. Ensure that the correct host directory is
mounted to `/etc/trident` in your Docker command.
- `-v /etc/pki:/etc/pki:ro`: Trident requires access to certificates in
`/etc/pki` to be able to authenticate with container registries, in which COSI
files may be stored. If the COSI file is stored locally or hosted at an
`http://` or `https://` URL which does not require authentication, it is not
required to mount this. In addition, note that Trident only requires read access
to this directory, which is why we recommend mounting with `ro`.
- `-v /var/lib/trident:/var/lib/trident`: This is the default location of the
Trident datastore and must be accessible to Trident.
- `-v /var/log:/var/log`: Trident logs and metrics are stored at
`/var/log/trident-full.log` and `/var/log/trident-metrics.jsonl`.
- `-v /:/host`: Trident requires access to the host machine's root filesystem
for operations such as device discovery, partitioning, and mounting and
unmounting filesystems.
- `-v /dev:/dev`: Trident must access devices.
- `-v /run:/run`: Trident makes use of various systemd services which require
access to `/run`.
- `-v /sys:/sys`: Trident makes use of various systemd services which require
access to `/sys`.
