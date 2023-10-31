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

### Building and validating

Build instructions: `cargo build`.

Build, check and and run UTs: `make`.

Code coverage: `make coverage`.

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

The Host Configuration contains the following sections:

- **management**: describes the management configuration of the host.
- **storage**: describes the storage configuration of the host.
- **imaging**: describes the imaging configuration of the host.
- **network**: describes the network configuration of the host.
- **osconfig**: describes the OS configuration of the host.

### Management

The Management configuration controls the installation of the Trident agent onto
the runtime OS. It contains a number of fields:

- **disable**: a boolean flag. When set to `true`, prevents Trident from being
  enabled on the runtime OS. In that case, the remaining fields are ignored.
- **self-upgrade**: a boolean flag that indicates whether Trident should upgrade
  itself. If set to `true`, Trident will replicate itself into the runtime OS
  prior to transitioning. This is useful during development to ensure the
  matching version of Trident is used. Defaults to `false`.
- **phonehome**: URL to reach out to when runtime OS networking is up, so
  Trident can report its status. If not specified, the value from the Trident
  configuration will be used. This is useful for debugging and monitoring
  purposes, say by an orchestrator.
- **datastore-path**: Describes where to place the datastore Trident will use to
  store its state. Defaults to `/var/lib/trident/datastore.sqlite`. Needs to end
  with `.sqlite`, cannot be an existing file and cannot reside on a read-only
  filesystem or A/B volume.

### Storage

Storage configuration describes the disks and partitions of the host that will
be used to store the OS and data. Not all disks of the host need to be captured
inside the Host Configuration, only those that Trident should operate on. The
configuration is divided into the following sections: **disks**, **raid** and
**mount-points**.

#### Disks

The **disks** section describes the disks of the host. Each disk is described by
the following fields:

- **id**: a unique identifier for the disk. This is a user defined string that
  allows to link the disk to what is consuming it and also to results in the
  Host Status.
- **device**: the device path of the disk. Points to the disk device in the
  host. It is recommended to use stable paths, such as the ones under
  `/dev/disk/by-path/` or [WWNs](https://en.wikipedia.org/wiki/World_Wide_Name).
- **partition-table-type**: the partition table type of the disk. Supported
  values are: `gpt`.
- **partitions**: a list of partitions that will be created on the disk. Each
  partition is described by the following fields:
  - **id**: a unique identifier for the partition. This is a user defined string
    that allows to link the partition to the mount points and also to results in
    the Host Status.
  - **type**: the type of the partition. Supported values are: `esp`, `root`,
    `root-verity` `swap`, `home`, `var`. These correspond to [Discoverable
    Partition
    Types](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/).
  - **size**: the size of the partition. Allowed values are:
    - `grow` to dynamically grow the partition to fill the remaining space on
      the disk.
    - A string with the following format: `<number>[<unit>]`. Supported units
      are: `K`, `M`, `G`, `T`. If no unit is specified, the number is
      interpreted as bytes. If a unit letter is specified, it corresponds to
      `KiB`, `MiB`, `GiB`, `TiB` respectively. Examples: `1G`, `10M`,
      `1000000000`.

TBD: At the moment, the partition table is created from scratch. In the future,
it will be possible to consume an existing partition table.

#### RAID

The **raid** section describes the RAID arrays for the host. All RAID array
definitions need to be specified in the **software** section nested in the
***raid** section. Each software RAID is described by the following fields:

 - **id**: a unique identifier for the RAID array. This is a user defined string
   also used for mounting the RAID array.
 - **name**: the name of the RAID array. This is used to reference the RAID
   array on the system. For example, `some-raid` will result in
   `/dev/md/some-raid` on the system.
 - **level**: the RAID level of the array. Supported and tested values are
   `raid0`, `raid1`. Other possible values yet to be tested are: `raid5`,
   `raid6`, `raid10`.
 - **devices**: a list of devices that will be used to create the RAID array.
   See the reference links for picking the right number of devices. Devices are
   partition ids from the `disks` section.
 - **metadata-version**: the metadata of the RAID array. Supported and tested
   values are `1.0`. Note that this is a string attribute.

The RAID array will be created using the `mdadm` package. During a clean
install, all the existing RAID arrays that are on disks defined in the host
configuration will be unmounted, and stopped.

The RAID arrays that are defined in the host configuration will be created, and
mounted if specified in `mount-points`.

To learn more about RAID, please refer to the [RAID
wiki](https://wiki.archlinux.org/title/RAID)

To learn more about `mdadm`, please refer to the [mdadm
guide](https://raid.wiki.kernel.org/index.php/A_guide_to_mdadm)

#### Mount Points

The **mount-points** section describes the mount points of the host. These are
used by Trident to update the `/etc/fstab` in the runtime OS to correctly mount
the volumes. Each mount point is described by the following fields:

- **path**: the path of the mount point. This is the path where the volume will
  be mounted in the runtime OS. For `swap` partitions, the path should be
  `none`.
- **target-id**: the id of the partition that will be mounted at this mount
  point.
- **filesystem**: the filesystem to be used for this mount point. This value
  will be used to format the partition.
- **options**: a list of options to be used for this mount point. These will be
  passed as is to the `/etc/fstab` file.

The resulting `/etc/fstab` is produced as follows:

- For each mount point, a line is added to the `/etc/fstab` file, if the `path`
  does not already exist in the `/etc/fstab` supplied in the runtime OS image.
  If the `path` already exists in the `/etc/fstab` supplied in the runtime OS,
  it will be updated to match the configuration provided in the Host
  Configuration mount points.
- If a mount point is not present in the Host Configuration, but present in the
  `/etc/fstab`, the line will be preserved as is in the `/etc/fstab`.

Note that you do not need to specify the mounts points, if your runtime OS
`/etc/fstab` carries the correct configuration already. In this case, Trident
will not modify the `/etc/fstab` file nor will it format the partitions.

### Imaging

Imaging configuration describes the filesystem images that will be used to
deploy onto the host. The configuration is divided into two sections: **images**
and **ab-update**.

#### Images

The **images** section describes the filesystem images that will be used to
deploy onto the host. Each image is described by the following fields:

- **url**: the URL of the image. Supported schemes are: `file`, `http`, `https`.
- **sha256**: the SHA256 checksum of the image. This is used to verify the
  integrity of the image. The checksum is a 64 character hexadecimal string.
  Temporarily, you can pass `ignored` to skip the checksum verification.
- **format**: the format of the image. Supported values are: `raw-zstd`.
- **target-id**: the id of the partition that will be used to store the image.

#### AB Update

Under development, initial logic for illustration purposes only.

The **ab-update** section describes the A/B Update configuration of the host.
This section is optional. If not present, A/B Update will not be configured on
the host. This section is described by the following fields:

- **volume-pairs**: a list of volume pairs that will be used for A/B Update.
  Each volume pair is described by the following fields:
  - **id**: a unique identifier for the volume pair. This is a user defined
    string that allows to link the volume pair to the results in the Host Status
    and to the mount points.
  - **volume-a-id**: the id of the partition that will be used as the A volume.
  - **volume-b-id**: the id of the partition that will be used as the B volume.

You can target the A/B Update volume pair from the `images` and `mount-points`
and Trident will pick the right volume to use based on the A/B Update state of
the host.

### Network

Network configuration describes the network configuration of the host. The
configuration format is matching the netplan v2 format.

### OS Config

OS Config describes the OS configuration of the host.

#### Users

The **users** section contains a configuration map with the users that will be
created on the host. The key of the map is the username.

**NOTE: CURRENTLY ONLY THE `root` USER IS SUPPORTED.**

Each user is described by the following fields:

- **password-mode**: How the password is provided, can be:
  - `locked`: the user is created with a locked password. This is the default
     value.
  - `dangerous-plain-text`: the password is provided in plain text.
  - `dangerous-hashed`: the password provided has already been already hashed.
- **password**: The password to be used for the user. (This is **not** used when
  `password-mode` is `locked`.)
- **ssh-keys**: The SSH keys to be added to the user. This is a list of strings.
  Note that to authenticate using the `root` user, you will need to modify the
  sshd config out of bound.

### Sample configuration

```yaml
host-configuration:
  management:
    self-upgrade: true
  storage:
    disks:
      - id: os
        device: /dev/disk/by-path/pci-0000:00:1f.2-ata-1.0
        partition-table-type: gpt
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
          - id: trident
            type: linux-generic
            size: 1G
          - id: raid-a
            type: linux-generic
            size: 1G
          - id: raid-b
            type: linux-generic
            size: 1G
    raid:
      software:
        - id: some_raid
          name: some-raid1
          level: raid1
          devices:
            - raid-a
            - raid-b
    mount-points:
      - path: /boot/efi
        target-id: esp
        filesystem: vfat
        options: ["umask=0077"]
      - path: /
        target-id: root
        filesystem: ext4
        options: ["defaults"]
      - path: /var/lib/trident
        target-id: trident
        filesystem: ext4
        options: ["defaults"]
      - path: none
        target-id: swap
        filesystem: swap
        options: ["sw"]
      - path: /mnt/raid
        target-id: some_raid
        filesystem: ext4
        options: ["defaults"]
  imaging:
    images:
      - url: file:///boot.raw.zst
        sha256: cd93c867cb0238fecb3bc9a268092526ba5f5b351bb17e5aab6fa0a9fc2ae4f8
        format: raw-zstd
        target-id: esp
      - url: file:///root.raw.zst
        sha256: fef89794407c89e985deed49c14af882b7abe425c626b0a1a370b286dfa4d28d
        format: raw-zstd
        target-id: root
    ab-update:
      volume-pairs:
        - id: root
          volume-a-id: root-a
          volume-b-id: root-b
  network:
    ethernets:
      vmeths:
        match:
          name: enp*
        dhcp4: true
    version: 2

  osconfig:
    users:
      root:
        password-mode: locked # default
        ssh-keys: 
          - <MY_PUBLIC_SSH_KEY>
```

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
