---
ArtifactType: executable, rpm.
Documentation: ./README.md
Language: rust
Platform: mariner
Stackoverflow: URL
Tags: comma,separated,list,of,tags
---

# Trident

Deployment and update Agent for Mariner OS, allowing for inplace image
deployments and atomic updates. Initial focus is on Bare Metal deployments, but
can be leveraged outside of that as well.

## Contents

- [Trident](#trident)
  - [Contents](#contents)
  - [Docs](#docs)
  - [Getting Started](#getting-started)
    - [Custom builds](#custom-builds)
    - [Pre-built artifacts](#pre-built-artifacts)
    - [Trident Environments](#trident-environments)
    - [Trident RPMs](#trident-rpms)
      - [Dependencies](#dependencies)
  - [Running Trident](#running-trident)
    - [Safety check](#safety-check)
  - [Trident Configuration](#trident-configuration)
  - [Host Configuration](#host-configuration)
    - [Documentation](#documentation)
    - [Schema](#schema)
    - [Sample](#sample)
    - [Validator](#validator)
  - [A/B Update](#ab-update)
    - [Getting Started with A/B Update](#getting-started-with-ab-update)
    - [TODO: Next Steps](#todo-next-steps)
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

## Docs

- [BOM Agnostic Single Node Provisioning
Architecture](https://microsoft.sharepoint.com/teams/COSINEIoT-ServicesTeam/Shared%20Documents/General/BareMetal/BOM%20Agnostic%20Single%20Node%20Provisioning%20Architecture.docx?web=1).
- [Trident Agent
  Design](https://microsoft.sharepoint.com/teams/COSINEIoT-ServicesTeam/Shared%20Documents/General/BareMetal/Trident%20Agent%20Design.docx?web=1)

## Getting Started

### Custom builds

If you want to build the bits yourself or leverage any custom build from our pipelines, please follow the [Deployment
instructions](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit?path=/README.md&_a=preview).

### Pre-built artifacts

This is generally more recommended for most users. You can download the latest
Trident release from the [releases wiki
page](https://dev.azure.com/mariner-org/ECF/_wiki/wikis/MarinerHCI.wiki/3306/Trident-Release).
And you can learn more how to integrate it with MIC for building the
runtime/target image and the provisioning image on the [BareMetal Platform Tools
wiki
page](https://dev.azure.com/mariner-org/ECF/_wiki/wikis/MarinerHCI.wiki/3607/BareMetal-Platform-Tools).

### Trident Environments

Trident can be run in two environments:

- Provisioning: Trident is run from the provisioning OS (management OS) to provision the target
  OS. The provisioning OS is typically a live OS running from a CD or USB stick.
  It can be also a live OS running from a network boot or from a preprovisioned
  bootstrap OS.

- Runtime: Trident is run from the target/runtime/application OS to apply and updates.

In both cases, Trident can be manually invoked, started using SystemD or run from a container.

### Trident RPMs

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

#### Dependencies

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

- `start-network`: Uses the `network` or `networkOverride` configuration (see
  below for details, loaded from `/etc/trident/config.yaml`) to configure
  networking in the currently running OS. This is mainly use to startup network
  during initial provisioning when default DHCP configuration is not sufficient.
- `run`: Runs Trident in the current OS. This is the main command to use to
  start Trident. Trident will load its configuration from
  `/etc/trident/config.yaml` and start applying the desired HostConfiguration.
  If you in addition pass `--status <path-to-output-file>`, Trident will write
  the resulting Host Status to the specified file.
- `get`: At any point in time, you can request to get the current Host Status
  using this command. This will print the HostStatus to standard output. If you
  in addition pass `--status <path-to-output-file>`, Trident will write the Host
  Status into the specified file instead.

For any of the commands, you can change logging verbosity from the default
`WARN` by passing `--verbosity` and appending one of the following values:
`OFF`, `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE`. E.g. `--verbosity DEBUG`.

Note that you can override the configuration path by setting the `--config`
parameter.

For debugging and troubleshooting, you can [view the full log of
Trident](./docs/How-To-Guides/View-Trident's-Background-Log.md).

### Safety check

Trident may destroy user data if run from dev machine or other system that is
not intended to be provisioned. To hopefully avoid this, Trident runs a safety
check before provisioning. The check ensures Linux has been booted from a
ramdisk, and terminates the provisioning process if not. It can be disabled by
creating a file named `override-trident-safety-check` in the root directory.

## Trident Configuration

This configuration file is used by the Trident agent to configure itself. It is
composed of the following sections:

- **allowedOperations**: a combination of flags representing allowed operations.
  This is a list of operations that Trident is allowed to perform on the host.
  Supported flags are:
   - **StageDeployment**: Trident will stage the changes required by the updated
    host configuration, including deployment of the new runtime OS image onto
    block devices during a clean install or an A/B update. However, Trident
    will not reboot the host into the newly deployed runtime OS. This is useful
    if you want to drive additional operations on the host outside of Trident
    or delay the reboot until a later point in time. After the new runtime OS
    image has been staged, Trident will update the host's status to
    DeploymentStaged.
  - **FinalizeDeployment**: Trident will reboot the host into the newly deployed
    runtime OS image to complete a clean install or A/B update. Trident will
    first manage the UEFI firmware variables, to ensure that post reboot, the
    firmware will boot into the updated runtime OS image. Then, Trident will
    set the host's servicing state to DeploymentFinalized and reboot. After the
    host comes back up, Trident will confirm that firmware booted from the
    correct block device and change the host's state to Ready. Otherwise, if a
    rollback into the provisioning OS or the old runtime image has occurred,
    Trident will notify the user and set the host's state to DeploymentFailed.

  You can pass multiple flags, separated by `|`. Example:
  `StageDeployment | FinalizeDeployment`. You can pass `''` to disable all
  operations, which would result in getting refreshed Host Status, but no
  operations performed on the host.
- **datastore**: if present, indicates the path to an existing datastore Trident
  should load its state from. This field should not be included when Trident is
  running from the provisioning OS.
- **phonehome**: optional URL to reach out to when networking is up, so Trident
  can report its status. This is useful for debugging and monitoring purposes,
  say by an orchestrator. Note that separately the updates to the Host Status
  can be monitored, once gRPC support is implemented. TODO: document the
  interface, for reference in the meantime
  [src/orchestrate.rs](src/orchestrate.rs).
- **networkOverride**: optional network configuration for the bootstrap OS. If
  not specified, the network configuration from Host Configuration (see below)
  will be used otherwise.
- **grpc**: If present (to make it present, add `listenPort` attribute which can
  be `null` for the default port 50051 or the port number to be used for
  incoming gRPC connections), this indicates that Trident should start a gRPC
  server to listen for commands. The protocol is described by
  [proto/trident.proto](proto/trident.proto). This only applies to the current
  run of Trident. During provisioning, you can control whether gRPC is enabled
  on the runtime OS via the `enableGrpc` field within the Trident Management
  section of the Host Configuration. TODO: implement and document authorization
  for accessing the gRPC endpoint.
- **waitForProvisioningNetwork**: USE WITH CAUTION!! IT WILL INCREASE BOOT TIMES
  IF THE NETWORK CONFIGURATION IS NOT PERFECT. (Only affects clean installs)
  When set to `true`, Trident will start `systemd-networkd-wait-online` to wait
  for the provisioning network to be up and configured before starting the
  provisioning flow. To avoid problems, only configure interfaces you know
  should work and are required for provisioning. Try to match by full name to
  avoid matching interfaces you don't want to. E.g. `eth0` instead of `eth*` to
  avoid matching `eth1` and `eth2` as well.

Additionally, to configure the host, the desired host configuration can be
provided through either one of the following options:

- **hostConfigurationFile**: path to the host configuration file. This is a YAML
  file that describes the host configuration in the Host Configuration format.
  See below details.
- **hostConfiguration**: describes the host configuration. This is the
  configuration that Trident will apply to the host (same payload as
  `hostConfigurationFile`, but directly embedded in the Trident configuration).
  See below details.
- **kickstartFile**: path to the kickstart file. This is a kickstart file that
  describes the host configuration in the kickstart format. WIP, early preview
  only. TODO: document what is supported.
- **kickstart**: describes the host configuration in the kickstart format. This
  is the configuration that Trident will apply to the host (same payload as
  `kickstartFile`, but directly embedded in the Trident configuration). WIP,
  early preview only.

## Host Configuration

Host Configuration describes the desired state of the host.

### Documentation

The full schema is available here:
[HostConfiguration.md](docs/Reference/Host-Configuration/API-Reference/HostConfiguration.md).

### Schema

The raw JSON Schema for Host configuration is here:
[host-config-schema.json](trident_api/schemas/host-config-schema.json)

### Sample

An example Host Configuration YAML MD file is available here:
[sample-host-configuration](docs/Reference/Host-Configuration/sample-host-configuration.md).
Additional raw YAML configuration samples are available in [Samples](docs/Reference/Host-Configuration/Samples).

### Validator

Trident supports the `validate` subcommand to validate a Host Configuration YAML.
See [Host Configuration Validation](docs/How-To-Guides/Host-Configuration-Validation.md).

## A/B Update

Currently, **a basic A/B update flow via direct streaming** is available with
Trident. Users can request Trident to perform the initial deployment and A/B
upgrades of a disk partition, a RAID array, or an encrypted volume that is part
of an A/B volume pair. The image has to be published as a local raw file
compressed using the zstd compression algorithm.

### Getting Started with A/B Update

First, the OS image payload needs to be made available for Trident to operate
on as a local file. For example, the OS image can be bundled with the installer
OS and referenced from the initial HostConfiguration as follows:

   ```yaml
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
      mountPoints:
        - path: /boot/efi
          targetId: esp
          filesystem: vfat
          options: ["umask=0077"]
        - path: /
          targetId: root
          filesystem: ext4
          options: ["defaults"]
      images:
        - url: file:///trident_cdrom/data/esp.rawzst
          sha256: e8c938d2bc312893fe5a230d8d92434876cf96feb6499129a08b8b9d97590231
          format: raw-zst
          targetId: esp
        - url: file:///trident_cdrom/data/root.rawzst
          sha256: f1373b6216fc1597533040dcb320d9e859da849d99d030ee2e8b6778156f82f3
          format: raw-zst
          targetId: root
      abUpdate:
        volumePairs:
          - id: root
            volumeAId: root-a
            volumeBId: root-b
   ```

In the sample HostConfiguration above, we're requesting Trident to create
**two copies of the root** partition, i.e., a volume pair with id root that
contains two partitions root-a and root-b, and to place an image in the raw
zstd format onto root. However, as mentioned, the user can create volume pairs
of different types. In particular, each volume pair can contain two disk
partitions of any type, two RAID arrays, or two encrypted volumes.

When the installation of the initial runtime OS is completed, the user will be
able to log or ssh into the baremetal host, or the VM simulating a BM host. The
user can now request an A/B update by applying an edited Trident HostConfig. To
do so, the user needs to update the **storage.images** section with the
information on the new OS images, including their local URL links and sha256
hashes.

- To overwrite the Trident HostConfig, the user can use the following command:

    ```bash
    cat > /etc/trident/config.yaml << EOF
    <body of the updated HostConfig>
    EOF
    ```

    After overwriting the HostConfiguration, the user needs to apply the
    HostConfig by restarting Trident with the following command:

    ```bash
    sudo systemctl restart trident.service
    ```

    The user can view the Trident logs live with the following command:

    ```bash
    sudo journalctl -u trident.service -f
    ```

When the A/B update completes and the baremetal host, or a VM simulating a BM
host, reboots, the user will be able to log or ssh back into the host. Now, the
user can view the changes to the system by fetching the host's status:
`trident get`.

The user can also use commands such as `blkid` and `mount` to confirm that the
volume pairs have been correctly updated and that the correct block devices
have been mounted at the designated mountpoints.

## dm-verity Support

Please review [API Documentation](#documentation) for low level details.

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

Unless `allowed-operations` are limited, upon completing the deployment, Trident
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
jq -n --rawfile hc input/hc.yaml '{ hostConfiguration: $hc, allowedOperations: "stageDeployment | finalizeDeployment" }' > command.json

# Issue gRPC request and pretty print the output as it is streamed back
evans --host <target-ip-adddress> --proto path/to/trident/proto/trident.proto cli call --file command.json UpdateHost | jq -r .status
```

## Development

- [Prerequisites](dev-docs/prerequisites.md)
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
