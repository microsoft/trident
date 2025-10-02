# Root Verity

Root verity is a specific utilization of Verity, an integral part of the kernel
that ensures that I/O for anything on the protected filesystem (in this case,
root: `/`) is verified against a known good state. This is achieved by creating
a hash tree of the root filesystem contents, which is then used to validate the
integrity of the data being accessed.

The hash tree is visualized in the
[kernel documentation](https://docs.kernel.org/admin-guide/device-mapper/verity.html)
like this, where the `root-hash` is the root node of the hash tree:

``` text
                            [   root    ]
                           /    . . .    \
                [entry_0]                 [entry_1]
               /  . . .  \                 . . .   \
    [entry_0_0]   . . .  [entry_0_127]    . . . .  [entry_1_127]
      / ... \             /   . . .  \             /           \
blk_0 ... blk_127  blk_16256   blk_16383      blk_32640 . . . blk_32767
```

Trident partners with Image Customizer to deploy images that have `root`
configured with Verity and a partition storing the `root-hash`.

## Use Image Customizer to Create a COSI File

To create a COSI file with `Root Verity` enabled, Image Customizer provides
some [guidance](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/concepts/verity.html).

At a high level, there are only a couple things that need to be configured:

1. In addition to the typical `root` partition definition, a `root-hash`
   partition is needed like this:

    ``` yaml
    storage:
      disks:
      - partitionTableType: gpt
        partitions:
        - label: root-data
          id: root-data
          size: 2G
        - label: root-hash
          id: root-hash
          size: 128M
    ```

2. The [verity](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/configuration/verity.html)
   section is required:

    ``` yaml
    verity:
    - id: root
      name: root
      dataDeviceId: root-data
      hashDeviceId: root-hash
      dataDeviceMountIdType: part-label
      hashDeviceMountIdType: part-label
    ```

3. Verity filesystems should be created as read-only:

    ``` yaml
    - deviceId: root
      type: ext4
      mountPoint:
        path: /
        options: defaults,ro
    ```

With these sections defined for `root`, Image Customizer will generate a COSI
file containing a `root-hash` partition and an OS with Root Verity enabled.

## Use Trident to Deploy the COSI File

Once you have a COSI file that enables `root verity`, Trident can be used to
deploy it during install or update.

Create a Trident host configuration file that aligns to the Image Customizer
COSI. Specifically:

1. Include `root-data` and `root-hash` partitions/filesystems

    ```yaml
    storage:
      disks:
      - id: os
        device: /dev/sda
        partitionTableType: gpt
        partitions:
        - id: root-data
          type: root
          size: 4G
        - id: root-hash
          type: root-verity
          size: 1G
    ```

2. Create `verity` section

    ```yaml
    storage:
      verity:
      - id: root
        name: root
        dataDeviceId: root-data
        hashDeviceId: root-hash
    ```
