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
  - [A/B Update](#ab-update)
    - [Getting Started with Systemd-Sysupdate](#getting-started-with-systemd-sysupdate)
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
[trident_api/docs/trident-api.md](trident_api/docs/trident-api.md).

An HTML version is available in `trident_api/docs/html/trident-api.html`

If you have GUI session on Linux, `make view-docs` may help open your browser
automatically. Otherwise you can open the docs
[directly](trident_api/docs/html/trident-api.html). Note that the HTML version
only works when directly opened using your browser, it will not work when opened
using the ADO repo browser or directly in VSCode. We are working on uploading
the docs to the engineering docs site.

You can use the `Live Server` VSCode extension to view the docs in VSCode. After
installing the extension, you can right click on the HTML file and select `Open
with Live Server`. This will open the docs in your default browser.
Alternatively, if you did not want to install the 3rd party extension, you could
just start your own local web server and open the HTML file in your browser,
e.g.: `python3 -m http.server --directory trident_api/docs/html/`.

### Schema

The raw JSON Schema for Host configuration is here:
[trident_api/docs/trident-api-schema.json](trident_api/docs/trident-api-schema.json)

### Sample

An example Host Configuration YAML file is available here:
[trident_api/docs/sample-host-configuration.yaml](trident_api/docs/sample-host-configuration.yaml)

## A/B Update

Currently, **a basic A/B update flow via systemd-sysupdate** is available with
Trident. The users are able to update the **root** partition and write to
**esp** partition that is part of an A/B volume pair. Other types of partitions
will be eligible for A/B update in a later iteration.

### Getting Started with Systemd-Sysupdate

First, the OS image payload needs to be made available for systemd-sysupdate to
operate on. To use the terms from the sysupdate documentation, the source image
can be published in the following two ways:

1. **regular-file**: The OS image can be bundled with the installer OS and
referenced from the initial HostConfiguration as follows:

   ```yaml
     storage:
       images:
         - url: file:///boot.raw.xz
           sha256:    e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
           format: raw-lzma
           targetId: esp
         - url: file:///root.raw.xz
           sha256:    e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
           format: raw-lzma
           targetId: root
       abUpdate:
         volumePairs:
           - id: root
             volumeAId: root-a
             volumeBId: root-b
           - id: esp
             volumeAId: esp-a
             volumeBId: esp-b
   ```

   In the sample HostConfiguration above, we're requesting Trident to create
   **two copies of the esp** partition, i.e., a volume pair with id esp that
   contains two partitions esp-a and esp-b, and to place an image in the raw
   lzma format onto esp. First of all, having an esp A/B volume pair is required
   for a successful boot post-update. Second of all, using systemd-sysupdate to
   write to a partition is valid as long as the block device target-id
   corresponds to a partition that is inside of an A/B volume pair. (This is
   because systemd-sysupdate expects 2+ partitions of the given type to do an
   update.) However, the actual A/B update of the esp partition is **not** fully
   supported since the basic e2e flow does not yet implement all the changes
   required to successfully **update the bootloader**. This distinction is very
   important.

2. **url-file**: The OS image can be referenced using remote URLs, at an
HTTP/HTTPS endpoint, e.g. by leveraging Azure blob storage. There are several
requirements per the systemd-sysupdate flow:

   1) Along with the payload, there needs to be **a SHA256SUMS manifest file**
   published in the same remote directory as the image partition files. E.g., if
   the directory contains root_v2.raw.xz, then SHA256SUMS needs to contain the
   following line:

        ```text
        <sha256 hash><2 whitespaces><name of the updated partition file>\n
        ```

   2) The image payload needs to be published with the **.xz extension**, by
   using the LZMA2 compression algorithm, so that systemd-sysupdate can
   decompress the image.

   3) Per current logic, the name of the image partition file corresponds to its
   **version**. Trident will extract the file name from the URL provided by the
   user in the Trident HostConfig and use it inside of the transfer config file,
   to communicate which version is requested from systemd-sysupdate. This means
   that the user needs to use consistent naming for partition files, so that the
   name of the new partition image will be read by systemd-sysupdate as a newer
   version. E.g., a convenient naming scheme could be the following: `<partition
   label/type>_v<version number>.raw.xz` For partition labels, it is recommended
   to use GPT partition type identifiers, as defined in the Type section of
   [systemd repart.d
   manual](https://www.man7.org/linux/man-pages/man5/repart.d.5.html).

   4) The storage.images section in the sample HostConfiguration provided above
   can be set in the following way, to request url-file images for the runtime
   OS:

      ```yaml
      storage:
        images:
          - url: <URL to the boot image>
            sha256: <sha256 hash>
            format: raw-lzma
            targetId: esp
          - url: <URL to the root image>
            sha256: <sha256 hash>
            format: raw-lzma
            targetId: root
      ```

When the installation of the initial runtime OS is completed, the user will be
able to log into the baremetal host, or the VM simulating a BM host. The user
can now request an A/B update by applying an edited Trident HostConfig. To do
so, the user needs to replace the data inside of the storage.images section, to
request to update **root** and write a new image to **esp**, via format
**raw-lzma**, from a new URL, with the sha256 hash taken from SHA256SUMS
published in the first step. For instance, the storage.images section of the new
HostConfig shown above can be changed in the following way:

```yaml
storage:
  images:
    - url: <URL to the updated version of the image>
      sha256: <sha256 hash>
      format: raw-lzma
      targetId: esp
    - url: <URL to the updated version of the image>
      sha256: <sha256 hash>
      format: raw-lzma
      targetId: root
```

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
host, reboots, the user will be able to log back into the host by using the same
credentials. Now, the user can view the changes to the system by displaying the
HostStatus, which is stored in the datastore: `cat
/var/lib/trident/datastore.sqlite`. The user can use commands such as `blkid`
and `mount` to confirm that the partitions have been correctly updated and that
the correct block devices have been mounted at the designated mountpoints, such
as /boot/efi and /.

### TODO: Next Steps

- After A/B update, Trident will be creating an **overlay** file system for the
data/state partitions. This is required so that certain folders, as required by
the user, can be read from and/or written to.
- The user will be able to request an update from a file that is published to
other backends. In the next iteration, Trident will support downloading OS image
payloads published as **OCI artifacts** on Azure Container Registry. Moreover,
based on the users' needs, other image formats might be supported in the future,
beyond raw Zstd and raw Lzma.
- To support downloading OCI artifacts and potentially, other backends, **a
hybrid A/B update** will be implemented: when the user provides a URL link that
systemd-sysupdate cannot correctly download from, Trident will independently
download the payload, decompress it, verify its hash, and point
systemd-sysupdate to the local file, to execute an A/B update. This means that
the overhead associated with generating and publishing the SHA256SUMS manifest
file can be lifted from the user.
- Trident will offer support to update the entire image, i.e. all types of
partitions and not just root, via systemd-sysupdate.
- Encryption and dm-verity will be supported.
- In the next iteration, e2e testing with Trident will be implemented. Moreover,
the next PR will document the performance metrics for the A/B update, such as
the total downtime.
- In the next iteration, Trident will support rollback, in case of an
interrupted or failed A/B update.
- Currently, the basic e2e A/B update flow is only successful when using kexec()
to reboot the system post-update. However, the next iteration will also support
using firmaware reboot, i.e., reboot() in Trident. A mechanism will be
implemented to point the firmware to the correct esp partition; now, although
the GRUB configs are correctly overwritten, the firmware still attempts to boot
into the A partition by default.

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
