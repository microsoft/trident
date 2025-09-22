
# Writing a Simple Host Configuration

In this tutorial, we will write a simple Host Configuration and use Trident to
validate it.

## Writing the Host Configuration

First, create a blank new YAML file called `conf.yaml`. This will be the file in
which we write our Host Configuration.

We will start with the `image` section of our Host Configuration. The `image`
section tells Trident where to source the COSI file from. The API asks for a
`url`, where the COSI file is actually located or hosted, and a `sha384` which
can contain either the keyword `ignored` or a SHA384 hash of the COSI file. To
learn more about COSI files, please see its [Reference
page](../Reference/COSI.md). You can also learn how to create a COSI file in
this tutorial on [Building a Deployable
Image](./Building-a-Deployable-Image.md). For the purposes of this tutorial,
assume we have a COSI file available at `http://localhost/regular.cosi`. We will
also skip calculating the SHA384 hash of our COSI file and instead use the
`ignored` keyword. We can now tell Trident this location:

```yaml
image:
  url: http://localhost/regular.cosi
  sha384: ignored
```

<div style="border: 2px solid #cc0000; padding: 10px; margin: 20px 0;">❗ To ensure the integrity of your COSI file, we strongly suggest calculating the SHA384 hash of your COSI file in production settings.</div>

Next, we will populate the `storage` section of our Host Configuration. The
`storage` section should describe the physical layout of the provisioned OS.
Three sections should be populated: `disks`, including information about the
`partitions` on each disk; `abUpdate` to specify which partitions should be
updated by Trident; and `filesystems`, which maps filesystems to partitions.

First, we will define the disks and partitions. In the disks section, we list
each physical disk and the partitions we want to create on it. Each disk needs a
unique `id`, its `device` path, and a `partitionTableType`. (Currently, Trident
only supports `gpt` partition tables). For each partition, we provide an `id`, a
(Discoverable
Partition)[https://uapi-group.org/specifications/specs/discoverable_partitions_specification/]
`type`, and its `size`.

For this tutorial, we'll set up a disk with a `root-a`, `root-b`, `esp`, `home`,
and `trident` partition. The `trident` partition will be used solely for storing
Trident's datastore. This must be a separate partition so that it is not
over-written during A/B Update.

```yaml
storage:
  disks:
    - id: os
      device: /dev/sda
      partitionTableType: gpt
      partitions:
        - id: root
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
```

Next, we'll configure A/B updates in the `abUpdate` section. For more detailed
information on A/B Updates, please reference the (How-To guide on A/B
Updates)[../How-To-Guides/Configure-an-ABUpdate-Ready-Host.md]. In this section,
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
partitions (or volume pairs) to mount points in the OS. The `deviceId` refers to
the `id` of a partition or a volume pair. For filesystems that are sourced from
the COSI file, we specify this with `source: image`. For filesystems that
Trident should newly create, we specify this with `source: new`. Lastly, each
filesystem must also have a `mountPoint`. If specific mount options are
necessary, as with the `esp` partition, we specify the filesystem as follows:

```yaml
- deviceId: esp
  source: image
  mountPoint: 
    path: /boot/efi
    options: umask=0077
```

If the default mount options are acceptible, then we can use a short cut and
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
mounted at `/var/lib/trident` by default.

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

we have now completed our Host Configuration! The final Host Configuration
should look as follows:

```yaml
image:
  url: http://NETLAUNCH_HOST_ADDRESS/files/regular.cosi
  sha384: ignored
storage:
  disks:
    - id: os
      device: /dev/disk/by-path/pci-0000:00:1f.2-ata-2
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
    - deviceId: trident
      source: new
      mountPoint: /var/lib/trident
    - deviceId: home
      source: new
      mountPoint: /home
    - deviceId: esp
      mountPoint:
        path: /boot/efi
        options: umask=0077
    - deviceId: root
      mountPoint: /
```

## Validating the Host Configuration with Trident

We will now use Trident to validate our Host Configuration with the following
command:

```bash
trident validate conf.yaml -v trace
```

The trace logs show how Trident builds a storage graph in order to validate the
Host Configuration we have passed it. For more information on validating a Host
Configuration, you can reference (this How-To
guide)[../How-To-Guides/Host-Configuration-Validation.md]. You should see a log
similar to the following:

`[INFO  trident::validation] Host Configuration is valid`

With this log, we know that we have successfully written a valid Trident Host
Configuration that can be used to provision an OS