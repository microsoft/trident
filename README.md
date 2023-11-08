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

Currently, **a basic A/B update flow via systemd-sysupdate** is available with
Trident. The users are able to update the **root** partition and write to
**esp** partition that is part of an A/B volume pair. Other types of partitions
will be eligible for A/B update in a later iteration.

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

##### Getting Started with Systemd-Sysupdate
- First, the OS image payload needs to be made available for systemd-sysupdate
to operate on. To use the terms from the sysupdate documentation, the source
image can be published in the following two ways:

1. **regular-file**: The OS image can be bundled with the installer OS and
referenced from the initial HostConfiguration as follows:

```yaml
  imaging:
    images:
      - url: file:///boot.raw.xz
        sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        format: raw-lzma
        target-id: esp
      - url: file:///root.raw.xz
        sha256: e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
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

- In the sample HostConfiguration above, we're requesting Trident to create
**two copies of the esp** partition, i.e., a volume pair with id esp that
contains two partitions esp-a and esp-b, and to place an image in the raw lzma
format onto esp. First of all, having an esp A/B volume pair is required for a
successful boot post-update. Second of all, using systemd-sysupdate to write to
a partition is valid as long as the block device target-id corresponds to a
partition that is inside of an A/B volume pair. (This is because
systemd-sysupdate expects 2+ partitions of the given type to do an update.)
However, the actual A/B update of the esp partition is **not** fully supported
since the basic e2e flow does not yet implement all the changes required to
successfully **update the bootloader**. This distinction is very important.

2. **url-file**: The OS image can be referenced using remote URLs, at an
HTTP/HTTPS endpoint, e.g. by leveraging Azure blob storage. There are several
requirements per the systemd-sysupdate flow:
1) Along with the payload, there needs to be **a SHA256SUMS manifest file**
published in the same remote directory as the image partition files. E.g., if
the directory contains root_v2.raw.xz, then SHA256SUMS needs to contain the
following line:
`<sha256 hash><2 whitespaces><name of the updated partition file>\n`
2) The image payload needs to be published with the **.xz extension**, by
using the LZMA2 compression algorithm, so that systemd-sysupdate can decompress
the image.
3) Per current logic, the name of the image partition file corresponds to its
**version**. Trident will extract the file name from the URL provided by the
user in the Trident HostConfig and use it inside of the transfer config file,
to communicate which version is requested from systemd-sysupdate. This means
that the user needs to use consistent naming for partition files, so that
the name of the new partition image will be read by systemd-sysupdate as a
newer version. E.g., a convenient naming scheme could be the following:
`<partition label/type>_v<version number>.raw.xz`
For partition labels, it is recommended to use GPT partition type identifiers,
as defined in the Type section of systemd repart.d manual:
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

- When the installation of the initial runtime OS is completed, the user will
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
`cat > /etc/trident/config.yaml <<EOF`
`<body of the updated HostConfig>`
`EOF`
After overwriting the HostConfiguration, the user needs to apply the HostConfig
by restarting Trident with the following command:
`sudo systemctl restart trident.service`.
The user can view the Trident logs live with the following command:
`sudo journalctl -u trident.service -f`.

- When the A/B update completes and the baremetal host, or a VM simulating a
BM host, reboots, the user will be able to log back into the host by using the
same credentials. Now, the user can view the changes to the system by
displaying the HostStatus, which is stored in the datastore:
`cat /var/lib/trident/datastore.sqlite`.
The user can use commands such as `blkid` and `mount` to confirm that the
partitions have been correctly updated and that the correct block devices
have been mounted at the designated mountpoints, such as /boot/efi and /.

##### TODO: Next Steps
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


### Network

Network configuration describes the network configuration of the host. The
configuration format is matching the netplan v2 format.

### OS Config

OS Config describes the OS configuration of the host.

#### Users

The **users** section contains a configuration map with the users that will be
created on the host. The key of the map is the username.

Each user is described by the following fields:

- **`groups`**: (Optional) The groups to be added to the user. This is a list of
  strings.
- **`ssh-keys`**: (Optional) The SSH keys to be added to the user. This is a list
  of strings.
- **`ssh_mode`**: (Optional) The SSH mode to be used for the user. Can be:
  - `block`: (default) the user is not allowed to SSH.
  - `key-only`: the user can SSH only with a key.

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
      my-new-user:
        # The password will be locked by default
        ssh-keys: 
          - <MY_PUBLIC_SSH_KEY>
        ssh-mode: key-only
  # Uncomment the following if you want to be able to use passwordless sudo using this user
  # post-install-scripts:
  # - content: 'echo "my-new-user ALL=(ALL) NOPASSWD:ALL" > /etc/sudoers.d/my-new-user'
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
