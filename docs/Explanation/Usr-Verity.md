
# Usr-Verity

Usr data integrity verification, or usr-verity, is a specific utilization
of [dm-verity](https://www.kernel.org/doc/html/latest/admin-guide/device-mapper/verity.html),
an integral part of the kernel that ensures that I/O for anything on the
protected filesystem (in this case, usr: `/usr`) is verified against a known
good state. This is achieved by creating a hash tree of the usr filesystem
contents, which is then used to validate the integrity of the data being
accessed.

The Merkle hash tree is visualized in the
[kernel documentation](https://docs.kernel.org/admin-guide/device-mapper/verity.html)
like this, where the `usr-hash` is the root node of the hash tree:

``` text
                            [   usr    ]
                           /    . . .    \
                [entry_0]                 [entry_1]
               /  . . .  \                 . . .   \
    [entry_0_0]   . . .  [entry_0_127]    . . . .  [entry_1_127]
      / ... \             /   . . .  \             /           \
blk_0 ... blk_127  blk_16256   blk_16383      blk_32640 . . . blk_32767
```

Trident partners with Image Customizer to deploy images that have `usr`
configured with dm-verity and a partition storing the `usr-hash`.

## Use Image Customizer to Create a COSI File

To create a COSI file with usr-verity enabled, Image Customizer provides some
[guidance](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/concepts/verity.html).

At a high level, there are only a couple things that need to be configured:

1. In addition to the typical `usr-data` partition definition, a `usr-hash`
   partition is needed like this:

    ``` yaml
    storage:
      disks:
      - partitionTableType: gpt
        partitions:
        - label: usr-data
          id: usr-data
          size: 2G
        - label: usr-hash
          id: usr-hash
          size: 128M
    ```

2. The [verity](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/configuration/verity.html)
   section is required:

    ``` yaml
    verity:
    - id: usr
      name: usr
      dataDeviceId: usr-data
      hashDeviceId: usr-hash
      dataDeviceMountIdType: part-label
      hashDeviceMountIdType: part-label
    ```

3. Usr-verity filesystems should be created as read-only:

    ``` yaml
    - deviceId: usr
      type: ext4
      mountPoint:
        path: /usr
        options: defaults,ro
    ```

4. Usr-verity requires some changes to support UKI rather than grub:

    ``` yaml
    os:
      kernelCommandLine:
        extraCommandLine:
        - rd.hostonly=0

    uki:
      kernels: auto

    previewFeatures:
    - uki
    ```

With these sections defined for `usr`, Image Customizer will generate a COSI
file containing a `usr-hash` partition and an OS with Usr Verity enabled.

## Use Trident to Deploy the COSI File

Once you have a COSI file that enables `Usr Verity`, Trident can be used to
deploy it during install or update.

Create a Trident Host Configuration file that aligns to the Image Customizer
COSI. Specifically:

1. Include `usr-data` and `usr-hash` partitions/filesystems

    ```yaml
    storage:
      disks:
      - id: os
        device: /dev/sda
        partitionTableType: gpt
        partitions:
        - id: usr-data
          type: usr
          size: 4G
        - id: usr-hash
          type: usr-verity
          size: 1G
    ```

2. Create [verity](../Reference/Host-Configuration/API-Reference/VerityDevice.md)
   section:

    ```yaml
    storage:
      verity:
      - id: usr
        name: usr
        dataDeviceId: usr-data
        hashDeviceId: usr-hash
    ```
