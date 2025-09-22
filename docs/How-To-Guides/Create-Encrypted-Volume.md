
# Create an Encrypted Volume

This how-to-guide explains how to create a new encrypted volume with Trident on [clean install](docs/How-To-Guides/Perform-Clean-Install), using the `encryption` configuration inside the Host Configuration API.Trident does **not** support adopting an existing encrypted volume or creating a new encrypted device on A/B update.

## Steps

1. Create a new device to encrypt using the Host Configuration, or identify an existing device. Trident supports encrypting devices of the following types:

   - Disk partition of a supported type.
   - Software RAID array, whose first disk partition is of a supported type.

   **Supported type** refers to any partition type, excluding a list of blocked types, as described in [the API doc on encrypted volumes](docs/Reference/Host-Configuration/API-Reference/EncryptedVolume.md).

1. Add a new encrypted volume to the `encryption` config under `volumes`, with these three **required** fields:

   - `id` is the id of the LUKS-encrypted volume to create. It must be non-empty and unique among the ids of all block devices in the host configuration. This includes the ids of all disk partitions, encrypted volumes, software RAID arrays, and A/B volume pairs.
   - `deviceName` is the name of an encrypted device to create under `/dev/mapper` when opening the volume. It should be a valid file name and unique among all encrypted volumes, as well as among the Device Mapper devices.
   - `deviceId` must correspond to a Trident-registered id of the device in the host configuration. In other words, it is the id of the partition or RAID array to encrypt. It also must be unique among the list of encrypted volumes.

   For example, the following configuration creates a new encrypted volume with id `web-encrypted` and device name `web`. It encrypts another block device, a partition with an id `web-partition`.

   ```yaml
   encryption:
     volumes:
       - id: enc-web-partition
         deviceName: luks-web-partition
         deviceId: web-partition
   ```

   The naming convention for encrypted volumes in Trident is to prefix the id of the partition or RAID array with `enc-<device_id>` to create the id of the encrypted volume, and prefix it with `luks-<device_id>` to create its device name.

1. Update the `encryption` configuration to include optional settings. For example, you can set a `recoveryKeyUrl` to read the recovery key from and choose `pcrs` to seal the encrypted volumes to. Remember that these settings apply to **all** encrypted volumes at once. More information about these settings can be found in [the API doc on encryption](docs/Reference/Host-Configuration/API-Reference/Encryption.md).

1. Run Trident to create the encrypted volume on clean install. Trident will:
   - Create the LUKS-encrypted volume on the specified device.
   - Generate encryption keys, or use the provided recovery keys, and seal them to the state of the TPM 2.0 device.
   - Make the encrypted volume available under `/dev/mapper/{deviceName}`.
