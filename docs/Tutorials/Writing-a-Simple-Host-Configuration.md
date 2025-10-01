
# Writing a Simple Host Configuration

## Introduction

In this tutorial, we will write a simple Host Configuration and use Trident to
validate it.

For a detailed breakdown of all available configurations, refer to the [Host
Configuration API
documentation](../Reference/Host-Configuration/API-Reference/HostConfiguration.md).
You can also view a complete [sample Host
Configuration](../Reference/Host-Configuration/Sample-Host-Configuration.md).

## Prerequisites

1. An OS with the Trident binary installed. To check if Trident is available on
   your OS, try running `trident --version`.
2. A COSI file. Please complete the tutorial on [Building A/B Update Images for
   Install and Update](./Building-AB-Update-Images-for-Install-and-Update.md) if
   you have not already.

## Instructions

First, create a new YAML file called `hostconf.yaml`. This will be the file in
which we write our Host Configuration.

### Step 1: Writing the Host Configuration

#### Image Section

We will start with the `image` section of our Host Configuration, which tells
Trident where to source the COSI file from. For a complete description of this
section, please reference the [API
documentation](../Reference/Host-Configuration/API-Reference/OsImage.md).

Additionally, to learn more about COSI files, please see its [Reference
page](../Reference/Composable-OS-Image.md). You can also learn how to create a
COSI file in this tutorial on [Building A/B Update Images for Install and
Update](./Building-AB-Update-Images-for-Install-and-Update.md).

The `image` section requires a `url`, where the COSI file is actually located or
hosted, and a `sha384` hash of the metadata of the COSI file. For the purposes
of this tutorial, assume we have a COSI file available at
`http://example.com/regular.cosi`. Please replace this URL with your actual COSI
file's URL. Note that in addition to the `http://` and `https://` URL schemes,
Trident also accepts the `file://` scheme for local files and the `oci://`
scheme for images stored in container registries. Please reference the [API
documentation](../Reference/Host-Configuration/API-Reference/OsImage.md) for
more information. In order to calculate the hash of the metadata of the COSI
file, we need to extract the `metadata.json` file and then calculate the SHA384
hash of the file:

```bash
tar -xvf artifacts/test-image/regular.cosi metadata.json
sha384sum metadata.json
```

We can now specify our OS image to Trident:

```yaml
image:
  url: http://example.com/regular.cosi
  sha384: <Calculated SHA384 hash>
```

#### Storage Section

Next, we will define the `storage` section, which describes the disk layout of
the target OS. Please reference the [API
Documentation](../Reference/Host-Configuration/API-Reference/Storage.md) for a
complete description of this section. Three sections should be populated:
`disks`, including information about the `partitions` on each disk; `abUpdate`
to specify which partitions should be serviced by Trident in a future A/B
update; and `filesystems`, which maps filesystems to partitions.

First, we will define the disks and partitions. In the disks section, we list
each disk and the partitions we want to create on it. Each disk needs a unique
`id`, its `device` path, and a `partitionTableType`. (Currently, Trident only
supports `gpt` partition tables). For each partition, we provide an `id`, a
[Discoverable
Partition](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/)
`type`, and its `size`. Full details on how to specify a partition can be found
in the [API
documentation](../Reference/Host-Configuration/API-Reference/Partition.md).

For this tutorial, we'll set up a disk with `esp`, `root-a`, `root-b`, `home`,
and `trident` partitions. The `trident` partition, used solely for storing the
Trident datastore, must be a separate partition to ensure it is not over-written
during an A/B update.

```yaml
storage:
  disks:
    - id: os
      device: /dev/sda
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
        - id: home
          type: home
          size: 1G
        - id: trident
          type: linux-generic
          size: 1G
```

Note that any other disks on the machine will be ignored by Trident, since they
are not listed in the Host Configuration. If you were to list another empty disk
(as below), the Trident would completely wipe the contents of this disk.

```yaml
  - id: disk2
    device: /dev/sdb
    partitionTableType: gpt
    partitions: []
```

While this tutorial uses device paths `/dev/sda` and `/dev/sdb`, in a production
setting it is best to use more predictable device paths (i.e.
`/dev/disk/by-id/...`) as kernel device naming can be unpredictable.

Next, we'll configure A/B servicing in the `abUpdate` section. For more detailed
information on A/B updates, please reference the [How-To guide on A/B
updates](../How-To-Guides/Configure-an-AB-Update-Ready-Host.md). In this
section, we define `volumePairs` that link two partitions together. Here, we'll
pair `root-a` and `root-b` as a single updatable volume named `root`.

```yaml
# ... (within the storage section)
  abUpdate:
    volumePairs:
      - id: root
        volumeAId: root-a
        volumeBId: root-b
```

Finally, we'll set up the `filesystems` section. This is where we map our
partitions (or A/B volume pairs) to mount points in the OS. Full details on how
to specify a filesystem can be found in the [API
documentation](../Reference/Host-Configuration/API-Reference/FileSystem.md). The
`deviceId` refers to the `id` of a partition or an A/B volume pair. For
filesystems that are sourced from the COSI file, we specify this with `source:
image`. For filesystems that Trident should newly create, we specify this with
`source: new`. Lastly, each filesystem must also have a `mountPoint`. If you
need to specify mount options, as with the `esp` partition, use the `path` and
`options` fields:

```yaml
- deviceId: esp
  source: image
  mountPoint: 
    path: /boot/efi
    options: umask=0077
```

If the default mount options are acceptable, then we can use a short cut and
only specify the `mountPoint` path, as with the root filesystem:

```yaml
- deviceId: root
  source: image
  mountPoint: /
```

Note that for the root filesystem, we use the `deviceId` `root`, instead of
`root-a` or `root-b`. This is because Trident will internally treat `root-a` and
`root-b` as one device under the ID `root`, since only one of `root-a` and
`root-b` is active at any time.

Lastly, we will create two new filesystems for the `home` and `trident`
partitions. Note that the filesystem on the `trident` partition should be
mounted at `/var/lib/trident` by default. Note that the path of the Trident
datastore may be changed with the [`trident`
API](../Reference/Host-Configuration/API-Reference/Trident.md).

By now, your `filesystems` section should look as follows:

```yaml
# ... (within storage section)
  filesystems:
    - deviceId: trident
      source: new
      mountPoint: /var/lib/trident
    - deviceId: home
      source: new
      mountPoint: /home
    - deviceId: esp
      source: image
      mountPoint:
        path: /boot/efi
        options: umask=0077
    - deviceId: root
      source: image
      mountPoint: /
```

We have now completed our Host Configuration! The final Host Configuration
should look as follows:

```yaml
image:
  url: http://example.com/regular.cosi
  sha384: <Calculated SHA384 hash>
storage:
  disks:
    - id: os
      device: /dev/sda
      partitionTableType: gpt
      partitions:
        - id: root-a
          type: root
          size: 8G
        - id: root-b
          type: root
          size: 8G
        - id: esp
          type: esp
          size: 1G
        - id: home
          type: home
          size: 1G
        - id: trident
          type: linux-generic
          size: 1G
  abUpdate:
    volumePairs:
      - id: root
        volumeAId: root-a
        volumeBId: root-b
  filesystems:
    - deviceId: esp
      source: image
      mountPoint:
        path: /boot/efi
        options: umask=0077
    - deviceId: root
      source: image
      mountPoint: /
    - deviceId: trident
      source: new
      mountPoint: /var/lib/trident
    - deviceId: home
      source: new
      mountPoint: /home
```

### Step 2: Validating the Host Configuration with Trident

We will now use Trident to validate our Host Configuration with the following
command:

```bash
trident validate hostconf.yaml -v trace
```

The trace logs show how Trident builds a storage graph in order to validate the
relationships between the filesystems and partitions in the Host Configuration
we have passed it. For more information on validating a Host Configuration, you
can reference [this How-To
guide](../How-To-Guides/Host-Configuration-Validation.md). You should see a log
similar to the following:

`[INFO  trident::validation] Host Configuration is valid`

This message confirms you have a valid Host Configuration.

Keep in mind that offline validation has limitations. First, since this
operation occurs offline Trident does not load the COSI file into memory and
therefore cannot validate the COSI file's contents against the `storage` section
of the Host Configuration. Second, Trident also cannot validate the SHA384 hash
of the COSI metadata file since this similarly requires loading the COSI file
into memory.

## Conclusion

Congratulations! You have successfully written and validated a complete Trident
Host Configuration.

In this tutorial, you learned how to:

- Define the OS image source using the `image` section.
- Describe a disk layout with disks, partitions, and filesystems in the
  `storage` section.
- Configure a volume pair for A/B updates.
- Use the `trident validate` command to check your configuration for errors.

### Next Steps

Now that you have a valid Host Configuration, you are ready to use it to
provision a device. Here are some tutorials to explore next:

- [Perform a clean install](./Onboard-a-VM-to-Trident.md)
- [Perform an A/B Update](./Performing-an-ABUpdate.md)
- [Run a custom script with Trident](./Running-Custom-Scripts.md)
