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
  - [gRPC Interface](#grpc-interface)
  - [Running from container](#running-from-container)
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

[Deployment
instructions](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit?path=/README.md&_a=preview).

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
  - **Update**: Trident will update the host based on the host configuration,
    but it will not transition the host to the new configuration. This is useful
    if you want to drive additional operations on the host outside of Trident.
  - **Transition**: Trident will transition the host to the new configuration,
    which can include rebooting the host. This will only happen if `Update` is
    also specified.

  You can pass multiple flags, separated by `|`. Example: `Update | Transition`.
  You can pass `''` to disable all operations, which would result in getting
  refreshed Host Status, but no operations performed on the host.
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
  on the runtime OS via the `enableGrpc` field within the Management section of
  the Host Configuration. TODO: implement and document authorization for
  accessing the gRPC endpoint.
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

An example Host Configuration YAML file is available here:
[sample-host-configuration](docs/Reference/Host-Configuration/sample-host-configuration.md)

### Validator

Trident supports the `validate` subcommand to validate a Host Configuration YAML.
See [Host Configuration Validation](docs/How-To-Guides/Host-Configuration-Validation.md).

## A/B Update

Currently, **a basic A/B update flow via direct streaming** is available with
Trident. Users can request Trident to perform the initial deployment and A/B
upgrades of a disk partition, a RAID array, or an encrypted volume that is part
of an A/B volume pair. The image has to be published as a local raw file
compressed into the ZSTD format.

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
          format: raw-zstd
          targetId: esp
        - url: file:///trident_cdrom/data/root.rawzst
          sha256: f1373b6216fc1597533040dcb320d9e859da849d99d030ee2e8b6778156f82f3
          format: raw-zstd
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
have been mounted at the designated mountpoints, such.

### TODO: Next Steps
In the future iterations, Trident will support the following additional
features:

- File-based A/B upgrade of the stand-alone ESP partition.
- Firmware reboot to complete the A/B update. Currently, the basic e2e A/B
update flow is only successful when using kexec to reboot the system after the
update.
- Rollback to the old valid OS image, in case of an interrupted or failed A/B
update.
- Decoupling of the A/B update into two steps: StageUpdate, which is the update
of the image, and Update, which includes the changes required to complete the
update and the reboot itself.
- Ability to select the reboot type for the next reboot, either kexec or
firmware reboot.

## gRPC Interface

If enabled, Trident will start a gRPC server to listen for commands. You can
interact with this server using the [evans gRPC
client](https://github.com/ktr0731/evans). Once installed, you can issue a gRPC
via the following commands:

```bash
# Generate command.json from input/hc.yaml
jq -n --rawfile hc input/hc.yaml '{ hostConfiguration: $hc, allowedOperations: "update | transition" }' > command.json

# Issue gRPC request and pretty print the output as it is streamed back
evans --host <target-ip-adddress> --proto path/to/trident/proto/trident.proto cli call --file command.json UpdateHost | jq -r .status
```

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
docker run --privileged -v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident -v /:/host --pid host trident/trident run
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
