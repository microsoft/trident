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

## Docs

- [BOM Agnostic Single Node Provisioning
Architecture](https://microsoft.sharepoint.com/teams/COSINEIoT-ServicesTeam/Shared%20Documents/General/BareMetal/BOM%20Agnostic%20Single%20Node%20Provisioning%20Architecture.docx?web=1).
- [Trident Agent
  Design](https://microsoft.sharepoint.com/teams/COSINEIoT-ServicesTeam/Shared%20Documents/General/BareMetal/Trident%20Agent%20Design.docx?web=1)

## Getting Started

[Deployment
instructions](https://dev.azure.com/mariner-org/ECF/_git/argus-toolkit?path=/README.md&_a=preview).

### Prerequisites

- Install [git](https://git-scm.com/downloads). E.g. `sudo apt install git`.
- Install Rust and Cargo: `curl https://sh.rustup.rs -sSf | sh`.
- Install `build-essential`, `pkg-config`, `libssl-dev`, `libclang-dev`, and
  `protobuf-compiler`. E.g. `sudo apt install build-essential pkg-config
  libssl-dev libclang-dev protobuf-compiler`.
- Clone the [Trident
  repository](https://mariner-org@dev.azure.com/mariner-org/ECF/_git/trident):
  `git clone https://mariner-org@dev.azure.com/mariner-org/ECF/_git/trident`.
- Change directory to the Trident repository: `cd trident`.
- (Only for changes to `trident_api`) Download documentation dependencies:
  
  ```bash
  make install-json-schema-for-humans
  ```

### Building and validating

Build instructions: `cargo build`.

Build, check and and run UTs: `make`.

Code coverage: `make coverage`.

Rebuild trident_api documentation: `make build-api-docs`.

### Updating documentation

After any change to trident_api, the documentation needs to be regenerated. Run:

```bash
make build-api-docs
```

## Trident configuration

This configuration file is used by the Trident agent to configure itself. It is
composed of the following sections:

- **allowed-operations**: a combination of flags representing allowed
  operations. This is a list of operations that Trident is allowed to perform on
  the host. Supported flags are:
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
- **network-override**: optional network configuration for the bootstrap OS. If
  not specified, the network configuration from Host Configuration (see below)
  will be used otherwise.

Additionally, to configure the host, the desired host configuration can be
provided through either one of the following options:

- **host-configuration-file**: path to the host configuration file. This is a
  YAML file that describes the host configuration in the Host Configuration
  format. See below details.
- **host-configuration**: describes the host configuration. This is the
  configuration that Trident will apply to the host (same payload as
  `host-configuration-file`, but directly embedded in the Trident
  configuration). See below details.
- **kickstart-file**: path to the kickstart file. This is a kickstart file that
  describes the host configuration in the kickstart format. WIP, early preview
  only. TODO: document what is supported.
- **kickstart**: describes the host configuration in the kickstart format. This
  is the configuration that Trident will apply to the host (same payload as
  `kickstart-file`, but directly embedded in the Trident configuration). WIP,
  early preview only.
- **grpc**: gRPC port to listen on, through which host configuration can be
  passed in once networking is up in the provisioning OS. Not yet implemented.

## Host Configuration

Host Configuration describes the desired state of the host.

### Documentation

The full schema is available here:
[trident_api/docs/trident-api.md](trident_api/docs/trident-api.md).

An HTML version is available in `trident_api/docs/html/trident-api.html`

(`make view-docs` may help open your browser automatically!)

### Schema

The raw JSON Schema for Host configuration is here: [trident_api/docs/trident-api-schema.json](trident_api/docs/trident-api-schema.json)

### Sample

An example Host Configuration YAML file is available here: [trident_api/docs/sample-host-configuration.yaml](trident_api/docs/sample-host-configuration.yaml)

## AB Update

Currently, **a basic A/B update flow via systemd-sysupdate** is available with
Trident. The users are able to update the **root** partition and write to
**esp** partition that is part of an A/B volume pair. Other types of partitions
will be eligible for A/B update in a later iteration.


### Getting Started with Systemd-Sysupdate

First, the OS image payload needs to be made available for systemd-sysupdate
to operate on. To use the terms from the sysupdate documentation, the source
image can be published in the following two ways:

1. **regular-file**: The OS image can be bundled with the installer OS and
referenced from the initial HostConfiguration as follows:

   ```yaml
     imaging:
       images:
         - url: file:///boot.raw.xz
           sha256:    e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
           format: raw-lzma
           target-id: esp
         - url: file:///root.raw.xz
           sha256:    e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
           format: raw-lzma
           target-id: root
       ab-update:
         volume-pairs:
           - id: root
             volume-a-id: root-a
             volume-b-id: root-b
           - id: esp
             volume-a-id: esp-a
             volume-b-id: esp-b
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
   version. E.g., a convenient naming scheme could be the following:
   `<partition label/type>_v<version number>.raw.xz` For partition labels, it is
   recommended to use GPT partition type identifiers, as defined in the Type
   section of systemd repart.d manual:
   https://www.man7.org/linux/man-pages/man5/repart.d.5.html.

   4) The Imaging section in the sample HostConfiguration provided above can be
   set in the following way, to request url-file images for the runtime OS:

      ```yaml
      imaging:
        images:
          - url: <URL to the boot image>
            sha256: <sha256 hash>
            format: raw-lzma
            target-id: esp
          - url: <URL to the root image>
            sha256: <sha256 hash>
            format: raw-lzma
            target-id: root
      ```

When the installation of the initial runtime OS is completed, the user will
be able to log into the baremetal host, or the VM simulating a BM host. The
user can now request an A/B update by applying an edited Trident HostConfig. To
do so, the user needs to replace the data inside of the Imaging section, to
request to update **root** and write a new image to **esp**, via format
**raw-lzma**, from a new URL, with the sha256 hash taken from SHA256SUMS
published in the first step. For instance, the Imaging section of the new
HostConfig shown above can be changed in the following way:

```yaml
imaging:
  images:
    - url: <URL to the updated version of the image>
      sha256: <sha256 hash>
      format: raw-lzma
      target-id: esp
    - url: <URL to the updated version of the image>
      sha256: <sha256 hash>
      format: raw-lzma
      target-id: root
```

- To overwrite the Trident HostConfig, the user can use the following command:

    ```bash
    cat > /etc/trident/config.yaml << EOF
    <body of the updated HostConfig>
    EOF
    ```

    After overwriting the HostConfiguration, the user needs to apply the HostConfig
    by restarting Trident with the following command:

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
HostStatus, which is stored in the datastore:
`cat /var/lib/trident/datastore.sqlite`. The user can use commands such as
`blkid` and `mount` to confirm that the partitions have been correctly updated
and that the correct block devices have been mounted at the designated
mountpoints, such as /boot/efi and /.

### TODO: Next Steps
- After A/B update, Trident will be creating an **overlay** file system for the
data/state partitions. This is required so that certain folders, as required by
the user, can be read from and/or written to.
- The user will be able to request an update from a file that is published to
other backends. In the next iteration, Trident will support downloading OS
image payloads published as **OCI artifacts** on Azure Container Registry.
Moreover, based on the users' needs, other image formats might be supported in
the future, beyond raw Zstd and raw Lzma.
- To support downloading OCI artifacts and potentially, other backends, 
**a hybrid A/B update** will be implemented: when the user provides a URL link
that systemd-sysupdate cannot correctly download from, Trident will
independently download the payload, decompress it, verify its hash, and point
systemd-sysupdate to the local file, to execute an A/B update. This means that
the overhead associated with generating and publishing the SHA256SUMS manifest
file can be lifted from the user.
- Trident will offer support to update the entire image, i.e. all types of
partitions and not just root, via systemd-sysupdate.
- Encryption and dm-verity will be supported.
- In the next iteration, e2e testing with Trident will be implemented.
Moreover, the next PR will document the performance metrics for the A/B update,
such as the total downtime.
- In the next iteration, Trident will support rollback, in case of an interrupted
or failed A/B update.
- Currently, the basic e2e A/B update flow is only successful when using
kexec() to reboot the system post-update. However, the next iteration will
also support using firmaware reboot, i.e., reboot() in Trident. A mechanism
will be implemented to point the firmware to the correct esp partition; now,
although the GRUB configs are correctly overwritten, the firmware still
attempts to boot into the A partition by default.

## Contributing

Please read our [CONTRIBUTING.md](CONTRIBUTING.md) which outlines all of our
policies, procedures, and requirements for contributing to this project.

## Versioning and changelog

We use [SemVer](http://semver.org/) for versioning. For the versions available,
see the [tags on this repository](link-to-tags-or-other-release-location).

It is a good practice to keep `CHANGELOG.md` file in repository that can be
updated as part of a pull request.

## Authors

yashpanchal@microsoft.com - RAID support

## License

This project is licensed under the < INSERT LICENSE NAME > - see the
[LICENSE](LICENSE) file for details

## Acknowledgments

- Hat tip to anyone whose code was used
- Inspiration
- etc
