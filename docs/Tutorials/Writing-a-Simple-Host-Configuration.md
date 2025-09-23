
# Writing a Simple Host Configuration

In this tutorial, we will write a simple Host Configuration and use Trident to
validate it.

Please also reference the API documentation for the [Host
Configuration](../Reference/Host-Configuration/API-Reference/HostConfiguration.md).
We also provide a [sample Host
Configuration](../Reference/Host-Configuration/Sample-Host-Configuration.md).

## Writing the Host Configuration

First, create a blank new YAML file called `hostconf.yaml`. This will be the
file in which we write our Host Configuration.

### Image Section

We will start with the `image` section of our Host Configuration. Please
reference the [API
documentation](../Reference/Host-Configuration/API-Reference/OsImage.md). The
`image` section tells Trident where to source the COSI file from.

To learn more about COSI files, please see its [Reference
page](../Reference/COSI.md). You can also learn how to create a COSI file in
this tutorial on [Building a Deployable
Image](./Building-a-Deployable-Image.md).

The API asks for a `url`, where the COSI file is actually located or hosted, and
a `sha384` which should contain the SHA384 hash of the metadata of the COSI
file. For the purposes of this tutorial, assume we have a COSI file available at
`http://example.com/regular.cosi`. Note that in addition to the `http://` and
`https://` URL schemes, Trident also accepts the `file://` scheme for local
files and the `oci://` scheme for images stored in container registries. Please
reference the [API
documentation](../Reference/Host-Configuration/API-Reference/OsImage.md) for
more information. In order to calculate the hash of the metadata of the COSI
file, we need to extract the `metadata.json` file and then calculate the SHA384
hash of the file:

```bash
tar -xvf artifacts/test-image/regular.cosi metadata.json
sha384sum metadata.json
```

 We can now tell Trident this location:

```yaml
image:
  url: http://example.com/regular.cosi
  sha384: <Calculated SHA384 hash>
```

### Storage Section

Next, we will populate the `storage` section of our Host Configuration. Please
reference the [API
Documentation](../Reference/Host-Configuration/API-Reference/Storage.md). The
`storage` section should describe the disk layout of the provisioned OS. Three
sections should be populated: `disks`, including information about the
`partitions` on each disk; `abUpdate` to specify which partitions should be
serviced by Trident in a future A/B update; and `filesystems`, which maps
filesystems to partitions.

First, we will define the disks and partitions. In the disks section, we list
each disk and the partitions we want to create on it. Each disk needs a unique
`id`, its `device` path, and a `partitionTableType`. (Currently, Trident only
supports `gpt` partition tables). For each partition, we provide an `id`, a
[Discoverable
Partition](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/)
`type`, and its `size`.

For this tutorial, we'll set up a disk with a `root-a`, `root-b`, `esp`, `home`,
and `trident` partition. The `trident` partition will be used solely for storing
Trident's datastore. This must be a separate partition so that it is not
over-written during A/B update.

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

Note that any other disks on the machine will be ignored by Trident, since it is
not listed in the Host Configuration. If you were to list another empty disk (as
below), the Trident would completely wipe the contents of this disk.

```yaml
  - id: disk2
    device: /dev/sdb
    partitionTableType: gpt
    partitions: []
```

Next, we'll configure A/B servicing in the `abUpdate` section. For more detailed
information on A/B updates, please reference the [How-To guide on A/B
Updates](../How-To-Guides/Configure-an-ABUpdate-Ready-Host.md). In this section,
we define `volumePairs` that link two partitions together. Here, we'll pair
`root-a` and `root-b` as a single updatable volume named `root`.

```yaml
# ... (within the storage section)
  abUpdate:
    volumePairs:
      - id: root
        volumeAId: root-a
        volumeBId: root-b
```

Finally, we'll set up the `filesystems` section. This is where we map our
partitions (or A/B volume pairs) to mount points in the OS. The `deviceId`
refers to the `id` of a partition or an A/B volume pair. For filesystems that
are sourced from the COSI file, we specify this with `source: image`. For
filesystems that Trident should newly create, we specify this with `source:
new`. Lastly, each filesystem must also have a `mountPoint`. If specific mount
options are necessary, as with the `esp` partition, we specify the filesystem as
follows:

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
`root-a` or `root-b`.

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

## Validating the Host Configuration with Trident

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

With this log line, we know that we have successfully written a valid Trident
Host Configuration that can be used to provision an OS.

Note that there are several important limitations to using Trident's offline
validation functionality. First, since this operation occurs offline Trident
does not load the COSI file into memory and therefore cannot validate the COSI
file's contents against the `storage` section of the Host Configuration. Second,
Trident also cannot validate the SHA384 hash of the COSI metadata file since
this similarly requires loading the COSI file into memory.
