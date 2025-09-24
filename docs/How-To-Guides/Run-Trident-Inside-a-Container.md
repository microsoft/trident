
# Run Trident Inside a Container

This guide explains how to run Trident inside a container.

## Goals

This guide will show you how to:

- Build a container image containing Trident.
- Load the image into your local container runtime.
- Run Trident from within a container.
- Understand the purpose of each flag and mounted directory required for Trident
  to function correctly.

## Prerequisites

1. This guide uses `docker` for all code snippets. However, the commands can be
   adapted to other tools.

## Instructions

### Step 1

Build the Trident container image using `make
artifacts/test-image/trident-container.tar.gz`. This Make target will build the
Trident RPMs (`make bin/trident-rpms/azl3.tar.gz`) and then use
[Dockerfile.runtime](../Dockerfile.runtime) to build the container image with
all the necessary dependencies. You can find a compressed form of containerized
Trident at `artifacts/test-image/trident-container.tar.gz`.

### Step 2

Load the Trident container image:

```bash
docker load --input trident-container.tar.gz
```

Depending on where you choose to place the Trident container image, change the
file path in the provided code sample.

### Step 3

Run Trident:

```bash
docker run --name trident_container \
           --pull=never \
           --rm \
           --privileged \
           -v /etc/trident:/etc/trident \
           -v /etc/pki:/etc/pki:ro \
           -v /var/lib/trident:/var/lib/trident \
           -v /var/log:/var/log \
           -v /:/host \
           -v /dev:/dev \
           -v /run:/run \
           -v /sys:/sys \
           --pid host \
           --ipc host \
           trident/trident:latest [TRIDENT VERB] /etc/trident/hostconf.yaml --verbosity TRACE
```

Note: By default, the Host Configuration should be placed inside
`/etc/trident/`. However, if you have placed your Host Configuration outside of
`/etc/trident/`, please mount the appropriate directory to `/etc/trident`:

```bash
-v <directory containing your Host Configuration>:/etc/trident`
```

Replace `[TRIDENT VERB]` with the desired verb. For a complete explanation of
the Trident CLI, please see the [Reference guide](../Reference/Trident-CLI.md).

#### Explanation of Docker Command

Trident must be run in `--privileged` mode so that it has access to devices on
the host, and allows Trident to perform operations such as partitioning disks
and creating filesystems. `--pid host` and `--ipc host` allow Trident to share
the host's PID namespace and IPC resources, necessary for communicating with
other system-level tools.

#### Mounted Volumes

`-v /etc/trident:/etc/trident`: By default, Trident expects to find the Host
Configuration in this directory. If the Host Configuration is not located in
this directory, this option is not required.

`-v /etc/pki:/etc/pki:ro`: Trident requires access to certificates in this
directory to be able to authenticate container registries, in which COSI files
may be stored. If the COSI file is stored or hosted locally, it is not required
to mount this. In addition, note that Trident only requires read access to this
directory, which is why we recommend mounting `ro`.

`-v /var/lib/trident:/var/lib/trident`: This is the default location of the
Trident datastore and must be accessible to Trident.

`-v /var/log:/var/log`: Trident logs and metrics are stored at
`/var/log/trident-full.log` and `/var/log/trident-metrics.jsonl`.

`-v /:/host`: Trident requires access to the root filesystem for operations such
as device discovery, partitioning, and mounting and unmounting filesystems.

`-v /dev:/dev`: Trident must access devices.

`-v /run:/run`: Trident makes use of various systemd services which require
access to `/run`.

`-v /sys:/sys`: Trident makes use of various systemd services which require
access to `/sys`.
