---
ArtifactType: executable, rpm.
Documentation: ./README.md
Language: rust
Platform: mariner
Stackoverflow: URL
Tags: comma,separated,list,of,tags
---

# Trident

Trident is a deployment and update agent for Azure Linux, allowing for inplace
image deployments and atomic updates. Initial focus is on Bare Metal
deployments, but can be leveraged outside of that as well.

## Contents

- [Trident](#trident)
  - [Contents](#contents)
  - [Background](#background)
  - [Getting Started](#getting-started)
    - [Developer Quickstart](#developer-quickstart)
    - [Download artifacts](#download-artifacts)
    - [Install Trident](#install-trident)
    - [Dependencies](#dependencies)
  - [Running Trident](#running-trident)
    - [Trident Environments](#trident-environments)
    - [Safety check](#safety-check)
  - [Trident Configuration](#trident-configuration)
    - [Host Configuration](#host-configuration)
    - [User Options](#user-options)
    - [Internal Fields](#internal-fields)
  - [A/B Update](#ab-update)
    - [Getting Started with A/B Update](#getting-started-with-ab-update)
  - [dm-verity Support](#dm-verity-support)
  - [Running from container](#running-from-container)
  - [Running from Azure VM](#running-from-azure-vm)
  - [gRPC Interface](#grpc-interface)
  - [Development](#development)
  - [Contributing](#contributing)
  - [Versioning and changelog](#versioning-and-changelog)
  - [Authors](#authors)
  - [License](#license)
  - [Acknowledgments](#acknowledgments)

## Background

- [BOM Agnostic Single Node Provisioning
Architecture](https://microsoft.sharepoint.com/teams/COSINEIoT-ServicesTeam/Shared%20Documents/General/BareMetal/BOM%20Agnostic%20Single%20Node%20Provisioning%20Architecture.docx?web=1).
- [Trident Agent
  Design](https://microsoft.sharepoint.com/teams/COSINEIoT-ServicesTeam/Shared%20Documents/General/BareMetal/Trident%20Agent%20Design.docx?web=1)

## Getting Started

### Developer Quickstart

Go to the [Quickstart Guide](dev-docs/quickstart.md) to get started with
development. This guide will help you set up your development environment and
build Trident.

### Download artifacts

You can download the latest Trident release from the [releases wiki
page](https://dev.azure.com/mariner-org/ECF/_wiki/wikis/MarinerHCI.wiki/3306/Trident-Release).
And you can learn more how to integrate it with MIC for building the
runtime/target image and the provisioning image on the [BareMetal Platform Tools
wiki
page](https://dev.azure.com/mariner-org/ECF/_wiki/wikis/MarinerHCI.wiki/3607/BareMetal-Platform-Tools).

(If you instead want to build the bits yourself or leverage any custom build from
our pipelines, please follow the [Deployment
instructions](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit?path=/README.md&_a=preview).)

### Install Trident

Trident is shipped as an RPM package. There are three main packages:

- `trident`: The main Trident package. It contains the Trident binary.
- `trident-service`: A SystemD service definition for Trident. This is only
  optional as you can also run Trident from a Docker container or invoke it
  directly on demand. This package depends on `trident` package.
- `trident-provisioning`. A SystemD service definition for Trident to be used
  during provisioning. This is only optional as you can also run Trident from a
  Docker container or invoke it directly on demand. This package depends on
  `trident` package. This starts Trident earlier in the boot process in order to
  setup networking on the provisioning OS in a way consistent with the
  provisioning of the target OS (though you can provision different network
  configuration for the provisioning OS if needed). If you use
  `trident-provisioning` during provisioning, you will also want to use
  `trident-service`, as only the latter triggers the actual provisioning.

### Dependencies

`trident` package has several optional dependencies. These are not enforced, as
not all customers will need all features. You will need to install these
dependencies into the OS where Trident is executed: provisioning or target OS if
running Trident directly on the OS or into the container image.

The following dependencies are optional:

- `netplan`: support for networking configuration. This supports `os.network`
  and `managementOs.network` sections of Host Configuration.
- `mdadm`: support for RAID configuration. This supports `storage.raid` section.
- `cryptsetup`, `tpm2-tools`: support for encrypted volumes. This supports `storage.encryption`
  section.
- `veritysetup`: support for dm-verity. This supports `storage.verity` section.

Trident also depends on more recent version of `systemd` compared to what is
available in Mariner/Azure Linux 2.0. For evaluation, you can use this
unsupported SystemD package:
[systemd-254-3.tar.gz](https://hermesimages.blob.core.windows.net/hermes-test/systemd-254-3.tar.gz).

## Running Trident

Trident can be automatically started using SystemD (see the [service
definitions](systemd)) or directly started manually. Trident support the
following commands (input as a command line parameter):

- `run`: Runs Trident in the current OS. This is the main command to use to
  start Trident. Trident will load its configuration from
  `/etc/trident/config.yaml` and start applying the desired HostConfiguration.
  If you in addition pass `--status <path-to-output-file>`, Trident will write
  the resulting Host Status to the specified file.
- `get`: At any point in time, you can request to get the current Host Status
  using this command. This will print the HostStatus to standard output. If you
  in addition pass `--status <path-to-output-file>`, Trident will write the Host
  Status into the specified file instead.
- `start-network`: Uses the `network` or `networkOverride` configuration (see
  below for details, loaded from `/etc/trident/config.yaml`) to configure
  networking in the currently running OS. This is mainly used to startup
  networking during initial provisioning when the default DHCP configuration is
  not sufficient.

For any of the commands, you can change logging verbosity from the default
`WARN` by passing `--verbosity` and appending one of the following values:
`OFF`, `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE`. E.g. `--verbosity DEBUG`.

Note that you can override the configuration path by setting the `--config`
parameter.

For debugging and troubleshooting, you can [view the full log of
Trident](./docs/How-To-Guides/View-Trident's-Background-Log.md).

### Trident Environments

Trident can be run in two environments:

- Provisioning: Trident is run from the provisioning OS (management OS) to provision the target
  OS. The provisioning OS is typically a live OS running from a CD or USB stick.
  It can be also a live OS running from a network boot or from a preprovisioned
  bootstrap OS.

- Runtime: Trident is run from the target/runtime/application OS to apply an update.

In both cases, Trident can be manually invoked, started using SystemD or run from a container.

### Safety check

Trident may destroy user data if run from dev machine or other system that is
not intended to be provisioned. To hopefully avoid this, Trident runs a safety
check before provisioning. The check ensures Linux has been booted from a
ramdisk, and terminates the provisioning process if not. It can be disabled by
creating a file named `override-trident-safety-check` in the root directory.

## Trident Configuration

Trident is controlled by an input file containing both the desired state of the
host and some configuration options. By default, this YAML file is read from
`/etc/trident/config.yaml` though the path can be overridden using the
`--config` command line option.

There is a `validate` subcommand that can be easily used to validate a config
file. It is intended to enable fast iteration and can be run from a dev machine
or other Linux system. Trident also supports validating a standalone Host
Configuration file. For more details, see [Host Configuration
Validation](docs/How-To-Guides/Host-Configuration-Validation.md).

The validator can check the configuration for syntax errors, but also for many
kinds of semantic errors. However, there are certain kinds of issues, like
referencing disks that do not exist, that cannot be caught by the validator.
Trident will catch these issues at runtime.

### Host Configuration

The desired state of the machine is described by passing one of the following:

- **hostConfiguration**: describes the host configuration. This is the
  configuration that Trident will apply to the host (same payload as
  `hostConfigurationFile`, but directly embedded in the Trident configuration).
- **hostConfigurationFile**: path to the host configuration file. This is a YAML
  file that describes the host configuration in the Host Configuration format.
- **kickstart**: describes the host configuration in the kickstart format. This
  is the configuration that Trident will apply to the host (same payload as
  `kickstartFile`, but directly embedded in the Trident configuration). WIP,
  early preview only.
- **kickstartFile**: path to the kickstart file. This is a kickstart file that
  describes the host configuration in the kickstart format. WIP, early preview
  only. TODO: document what is supported.

For more details on the Host Configuration format:

- An example Host Configuration YAML MD file is available here:
[Sample-Host-Configuration.md](docs/Reference/Host-Configuration/Sample-Host-Configuration.md).

- Additional raw YAML configuration samples are available in [Samples](docs/Reference/Host-Configuration/Samples).

- The full schema is available here:
[HostConfiguration.md](docs/Reference/Host-Configuration/API-Reference/HostConfiguration.md).

- And also as a JSON Schema here:
[host-config-schema.json](trident_api/schemas/host-config-schema.json)

### User Options

- **allowedOperations**: a list of flags representing allowed operations.
  This is a set of operations that Trident is allowed to perform on the host.
  Supported flags are:
  - **stage**: Trident will stage a new runtime OS as required by the updated
    host configuration. However, Trident will not reboot the host into the newly
    stage runtime OS. This is useful if you want to drive additional operations
    on the host outside of Trident or delay the reboot until a later point in
    time. After the new runtime OS image has been staged, Trident will update
    the host's status to Staged.
  - **finalize**: Trident will reboot the host into the newly staged runtime
    OS image to finalize a clean install or A/B update. Trident will first
    manage the UEFI firmware variables, to ensure that post reboot, the
    firmware will boot into the updated runtime OS image. Then, Trident will
    set the host's servicing state to Finalized and reboot. After the host
    comes back up, Trident will confirm that firmware correctly booted from the
    updated runtime OS image and change the host's state to Provisioned.
    Otherwise, if the host failed to boot from the expected device and instead,
    booted from another device, Trident will issue an error to the user and set
    the host's servicing state to CleanInstallFailed or AbUpdateFailed.

  You can pass one, multiple, or no flags as a YAML list, for example:

    ```yaml
    # Inline List
    allowedOperations: [stage, finalize]

    # Inline list, just one value
    allowedOperations: [finalize]

    # Multiline list
    allowedOperations:
      - stage
      - finalize

    # Multiline list, just one value
    allowedOperations:
      - finalize

    # No operations
    allowedOperations: []
    ```

  When no operations are allowed, Trident will refresh the Host Status, but no
  operations will be performed on the host.
- **phonehome**: optional URL to reach out to when networking is up, so Trident
  can report its status. This is useful for debugging and monitoring purposes,
  say by an orchestrator. Note that separately the updates to the Host Status
  can be monitored, once gRPC support is implemented. TODO: document the
  interface, for reference in the meantime
  [src/orchestrate.rs](src/orchestrate.rs).
- **networkOverride**: optional network configuration for the bootstrap OS. If
  not specified, the network configuration from Host Configuration (see below)
  will be used otherwise.
- **waitForProvisioningNetwork**: USE WITH CAUTION!! IT WILL INCREASE BOOT TIMES
  IF THE NETWORK CONFIGURATION IS NOT PERFECT. (Only affects clean installs)
  When set to `true`, Trident will start `systemd-networkd-wait-online` to wait
  for the provisioning network to be up and configured before starting the
  provisioning flow. To avoid problems, only configure interfaces you know
  should work and are required for provisioning. Try to match by full name to
  avoid matching interfaces you don't want to. E.g. `eth0` instead of `eth*` to
  avoid matching `eth1` and `eth2` as well.

<!-- There is also a grpc field, but it is not enabled in release builds -->

### Internal Fields

- **datastore**: if present, indicates the path to an existing datastore Trident
  should load its state from. This field should not be included when Trident is
  running from the provisioning OS. Trident interprets this field to mean that
  it is running from an already provisioned system and thus should perform
  updates rather than a clean install.

## A/B Update

Trident now offers **A/B update** via direct image streaming. Users can request
Trident to perform the initial deployment and A/B update of a disk partition, a
RAID array, or an encrypted volume that is part of an A/B volume pair. The
image has to be published as a local raw file compressed using the zstd
compression algorithm.

A key feature of A/B update with Trident is that **staging of new OS images**
**is decoupled from the reboot into the image**. In other words, the user can
first request Trident to stage deployment and then, separately, to finalize it.
After the new image has been staged, the user can repeatedly request to have
another OS image staged instead, before requesting to boot into the updated OS
image.

This decoupled logic is implemented for **both clean install and A/B update.**
This is achieved by splitting `allowedOperations`, where the user defines which
actions are permitted/desired, into `stage` and `finalize`.

### Getting Started with A/B Update

First, the OS image payload needs to be made available for Trident to operate
on as a local file. For example, the OS image can be bundled with the installer
OS and referenced from the initial host configuration as follows:

```yaml
hostConfiguration:
  storage:
    disks:
      - id: os
        device: /dev/disk/by-path/pci-0000:00:1f.2-ata-2
        partitionTableType: gpt
        partitions:
          - id: esp
            type: esp
            size: 1G
          - id: root-a
            type: root
            size: 8G
          - id: root-b
            type: root
            size: 8G
          - id: swap
            type: swap
            size: 2G
          - id: home
            type: home
            size: 1G
          - id: trident
            type: linux-generic
            size: 1G
      - id: disk2
        device: /dev/disk/by-path/pci-0000:00:1f.2-ata-3
        partitionTableType: gpt
        partitions: []
    abUpdate:
      volumePairs:
        - id: root
          volumeAId: root-a
          volumeBId: root-b
    filesystems:
      - deviceId: swap
        type: swap
      - deviceId: trident
        type: ext4
        mountPoint:
          path: /var/lib/trident
          options: defaults
      - deviceId: home
        type: ext4
        mountPoint:
          path: /home
          options: defaults
      - deviceId: esp
        type: vfat
        source:
          type: esp-image
          url: file:///trident_cdrom/data/esp.rawzst
          sha256: ignored
          format: raw-zst
        mountPoint:
          path: /boot/efi
          options: umask=0077
      - deviceId: root
        type: ext4
        source:
          type: image
          url: file:///trident_cdrom/data/root.rawzst
          sha256: ignored
          format: raw-zst
        mountPoint:
          path: /
          options: defaults
  scripts:
    postConfigure:
      - name: testing-privilege
        runOn:
          - clean-install
          - ab-update
        content: echo 'testing-user ALL=(ALL:ALL) NOPASSWD:ALL' > /etc/sudoers.d/testing-user
  os:
    network:
      version: 2
      ethernets:
        vmeths:
          match:
            name: enp*
          dhcp4: true
    users:
      - name: testing-user
        sshPublicKeys: []
        sshMode: key-only
```

In the sample host configuration above, we're requesting Trident to create
**two copies of the root** partition, i.e., a volume pair with id `root` that
contains two partitions `root-a` and `root-b`, and to place an image in the raw
zstd format onto `root`. However, as mentioned, the user can create volume pairs
of different types. In particular, each volume pair can contain two disk
partitions of any type except for ESP, two RAID arrays, or two encrypted volumes.

When the installation of the initial runtime OS is completed, the user will be
able to log or ssh into the baremetal host, or the VM simulating a BM host. The
user can now request an A/B update by applying an edited Trident host config.
To do so, the user needs to update the `filesystems` section with the info on
the new OS images, including their local URL links and sha256 hashes.

- To overwrite the Trident HostConfig, the user can either use vim or the sed
command, for example:

    ```bash
    sed -i 's|file:///trident_cdrom/data/esp.rawzst|<local_url>/esp_v2.rawzst|' /etc/trident/config.yaml
    sed -i 's|file:///trident_cdrom/data/root.rawzst|<local_url>/root_v2.rawzst|' /etc/trident/config.yaml
    ```

- After overwriting the host configuration, the user needs to apply the new
host config by restarting Trident with the following command:

    ```bash
    sudo systemctl restart trident.service
    sudo journalctl -u trident.service -f
    ```

  or:

    ```bash
    sudo trident run -v trace
    ```

- When the A/B update completes and the baremetal host, or a VM simulating a BM
host, reboots, the user will be able to log or ssh back into the host. Now, the
user can view the changes to the system by fetching the host's status:
`trident get`. The user can also use commands such as `blkid` and `mount` to
confirm that the volume pairs have been correctly updated and that the correct
block devices have been mounted at the designated mountpoints.

- If the user wants to separately stage or finalize a clean install or an A/B
update, `allowedOperations` also need to be updated, in addition to the image
info:

1. To only stage a new deployment, update the image info and set:
   `allowedOperations: [stage]`.
1. To only finalize the staged deployment, set: `allowedOperations: finalize`.
1. To both stage a new deployment and then immediately finalize it, update the
image info and set: `allowedOperations: [stage, finalize]`.

## dm-verity Support

Please review [API
Documentation](docs/Reference/Host-Configuration/Host-Configuration.md) for low
level details.

Specifically, you need to include `verity` under `storage` in
`HostConfiguration`. Currently, only `root` verity is supported (`deviceName`
needs to be `root` and the verity block device needs to be mounted at `/`).
Mount point needs to point to the verity block device, not the underlying data
block device. It also needs to be mounted read only.

When you choose to use verity, you will also need to ensure that:

- Trident datastore is stored on a separate read/write volume, that is not part
  of A/B update. By default, the datastore is stored in `/var/lib/trident`.
- `/var/lib/trident-overlay` (fixed path at the moment) is a mount point for
  another read/write volume. If you are also using A/B update blocks, this R/W
  volume needs to be passed through A/B block as well. This is used by Trident
  to store the configuration it generates for the target OS (it holds and
  overlay that gets mounted read only at `/etc`).
- You might also include `/var/lib` and `/var/log` RW volumes in order to allow
  for base services to write to disk. These can be redirected as part of MIC
  image constructions. Alternatively, you can redirect `/var` to a writable
  volume.
- Note that SSH will not start if `/etc/ssh` is read only. You can update SSH
  config or mount an overlay using a script included by MIC.
- If you use A/B update blocks, the recommended approach is to put any RW
  volumes behind A/B update blocks, to ensure clean separation between A/B
  instances.

## Running from container

Trident can be run from a container. To build the container, run:

```bash
make docker-build
```

If you want to use your own `Dockerfile`, you can use
[Dockerfile.runtime](Dockerfile.runtime) as a sample starting point. To run Trident successfully
from a container, make sure you set this as part of your `Dockerfile`:

```Dockerfile
DOCKER_ENVIRONMENT=true
```

Update `/etc/trident/config.yaml` with the desired configuration.

To run Trident using a docker container, run:

```bash
docker run --privileged -v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident -v /:/host -v /dev:/dev -v /run:/run -v /sys:/sys --pid host trident/trident run
```

## Running from Azure VM

Please note, while this has been manually tested, it is not generally supported.

You can start Trident from an Azure VM, perhaps for testing use case. You will
need to create Generation 2 VM, as Trident requires UEFI boot. You will also
want to include additional data disk, where Trident can deploy the
target/runtime OS to. For simple installations, 16GB disk should be sufficient.

You can boot from Ubuntu and start Trident in a container, you can use Mariner
gallery image and then you can run Trident natively or from a container. Or you
can upload your custom provisioning OS image first and boot from that. In either
case, the starting OS will act as the provisioning OS for Trident.

Please use Trident RPMs from our release page (or build your own) to deploy
them, if you dont want to build your own container or use a prebuilt
provisioning OS image. You will really only need the `trident` RPM, along with
any optional dependencies, depending on the features you are planning to use.
Then, you need to add your Host Configuration to `/etc/trident/config.yaml` (or
any other custom location and pass it explicitly to Trident) and invoke `sudo
trident run` to apply it. Note that you should not include the current OS disk
into the Host Configuration, only to the data disk (otherwise Trident would try
to reformat the current OS disk). Also to note, Trident is trying to do its best
to prevent data loss. As such, the current implementation will not allow to run
Trident from a non-live OS. To override this, create an empty override file
`sudo touch /override-trident-safety-check`.

Unless `allowedOperations` are limited, upon completing the deployment, Trident
will reboot the VM into the new OS.

## gRPC Interface

Please note, gRPC interface is in an early preview, does not support
authentication and is not generally yet supported.

If enabled, Trident will start a gRPC server to listen for commands. You can
interact with this server using the [evans gRPC
client](https://github.com/ktr0731/evans). Once installed, you can issue a gRPC
via the following commands:

```bash
# Generate command.json from input/hc.yaml
jq -n --rawfile hc input/hc.yaml '{ hostConfiguration: $hc, allowedOperations: "[stage, finalize]" }' > command.json

# Issue gRPC request and pretty print the output as it is streamed back
evans --host <target-ip-adddress> --proto path/to/trident/proto/trident.proto cli call --file command.json UpdateHost | jq -r .status
```

## Development

- [Quickstart Guide](dev-docs/quickstart.md)
- [Building and Validating](dev-docs/building-validating.md)
- [Code Coverage](dev-docs/code-coverage.md)
- [Updating Docs](dev-docs/updating-docs.md)
- [Testing](dev-docs/testing.md)

## Contributing

Please read our [CONTRIBUTING.md](CONTRIBUTING.md) which outlines all of our
policies, procedures, and requirements for contributing to this project.

## Versioning and changelog

We use [SemVer](http://semver.org/) for versioning. For the versions available,
see the [tags on this repository](link-to-tags-or-other-release-location).

It is a good practice to keep `CHANGELOG.md` file in repository that can be
updated as part of a pull request.

## Authors

[yashpanchal@microsoft.com](mailto:yashpanchal@microsoft.com) - RAID support

## License

This project is licensed under the < INSERT LICENSE NAME > - see the
[LICENSE](LICENSE) file for details

## Acknowledgments

- Hat tip to anyone whose code was used
- Inspiration
- etc
