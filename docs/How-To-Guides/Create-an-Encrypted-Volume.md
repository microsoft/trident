
# Create an Encrypted Volume

This how-to-guide explains how to create a new encrypted volume with Trident on clean install, using the `encryption` configuration inside the Host Configuration API. Currently, Trident does **not** support adopting an existing encrypted volume or creating a new encrypted device on A/B update.

## Steps

1. Create a new device to encrypt using the host configuration, or identify an existing device. Right now, Trident supports encrypting devices of the following types:

   - Disk partition of a supported type.
   - Software RAID array, whose first disk partition is of a supported type.

   **Supported type** refers to any partition type, excluding `root` and `efi` partitions.

1. Add a new encrypted volume to the `encryption` config under `volumes`, with these three **required** fields:

   - `id` is the id of the LUKS-encrypted volume to create. It must be non-empty and unique among the ids of all block devices in the host configuration. This includes the ids of all disk partitions, encrypted volumes, software RAID arrays, and A/B volume pairs.
   - `deviceName` is the name of the device to create under `/dev/mapper` when opening the volume. It should be a valid file name and unique among the list of encrypted volumes.
   - `deviceId` must correspond to a Trident-registered id of the device in the host configuration. In other words, it is the id of the partition or RAID array to encrypt. It also must be unique among the list of encrypted volumes.

   For example, the following configuration creates a new encrypted volume with id `web-encrypted` and device name `web`. It encrypts another block device, a partition or a RAID array, with an id `web-partition`.

   ```yaml
   encryption:
     volumes:
       - id: web-encrypted
         deviceName: web
         deviceId: web-partition
   ```

1. Optionally, update the `encryption` configuration to modify other settings. In particular, you can set a `recoveryKeyUrl` to read the recovery key from and choose `pcrs` to seal the encrypted volumes to. Remember that these settings apply to **all** encrypted volumes at once.

1. Run Trident to create the encrypted volume on clean install. Trident will:
   - Create the LUKS-encrypted volume on the specified device.
   - Generate encryption keys, or use the provided recovery keys, and seal them to the state of the TPM 2.0 device.
   - Make the encrypted volume available under `/dev/mapper/{deviceName}`.
