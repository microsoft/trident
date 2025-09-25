
# Create an Encrypted Volume

This guide explains how to create a new [encrypted volume](../Reference/Host-Configuration/API-Reference/EncryptedVolume.md) on [clean install](../Reference/Glossary.md/#clean-install) with Trident, using the Host Configuration API.

## Goals

By following this guide, you will:

1. Declare an encrypted volume using the Host Configuration API.
1. Configure an encrypted volume to be mounted at specified mount points in the target OS.
1. Set optional settings: recovery key and PCR encryption.
1. Create an encrypted volume on the target OS with Trident.

This guide will not cover adopting an existing encrypted volume in the [`offline-init`](../Explanation/Offline-Init.md) scenario or creating a new encrypted device on A/B updates, as Trident does **not** support these features.

## Prerequisites

1. A host that has not yet been serviced by Trident.
1. A host configuration with the basic structure, including the [`storage`](../Reference/Host-Configuration/API-Reference/Storage.md) section.

## Steps

### Step 1: Create Devices to Encrypt

1. Create a device to encrypt using the Host Configuration API. Trident supports encrypting devices of the following types:

   - Disk partition of a supported type.
   - Software RAID array, whose first disk partition is of a supported type.[^1] [This how-to guide](./Create-a-RAID-Array.md) outlines how to create a new RAID array.

[^1]: **Supported type** refers to any partition type, excluding a list of blocked types, as described in [the API doc on encrypted volumes](../Reference/Host-Configuration/API-Reference/EncryptedVolume.md).

### Step 2: Add `encryption` Configuration

1. Inside the host configuration, under `storage`, add a new encrypted volume to the `encryption.volumes` section, completing these three **required** fields:

   - `id` is the ID of the LUKS-encrypted volume to create. It must be non-empty and unique among the IDs of all block devices in the host configuration. This includes the IDs of all disk partitions, encrypted volumes, software RAID arrays, and A/B volume pairs.
   - `deviceName` is the name of an encrypted device to create under `/dev/mapper` when opening the volume. It should be a valid file name and unique among all encrypted volumes, as well as among the Device Mapper devices.
   - `deviceId` must correspond to a Trident-registered ID of the device in the host configuration. In other words, it is the ID of the partition or RAID array to encrypt. It also must be unique among the list of encrypted volumes.

   For example, the following configuration creates a new encrypted volume with ID `enc-web-partition` and device name `luks-web-partition`. It encrypts another block device, a partition with an ID `web-partition`.

   ```yaml
   storage:
      encryption:
         volumes:
            - id: enc-web-partition
               deviceName: luks-web-partition
               deviceId: web-partition
   ```

   The naming convention for encrypted volumes in Trident is to prefix the ID of the partition or RAID array with `enc-<device_id>` to create the ID of the encrypted volume, and prefix it with `luks-<device_id>` to create its device name.

1. If the encrypted volume needs to be mounted, the `storage.filesystems` configuration must be updated to request that.

   ```yaml
   storage:
      filesystems:
         - deviceId: enc-web-partition
            source: new
            mountPoint: /web
   ```

   For example, this configuration describes that the encrypted volume with ID `enc-web-partition` from above should be mounted at `/web`, by creating a new filesystem. [The API on filesystems](../Reference/Host-Configuration/API-Reference/FileSystem.md) contains more information on the flesystems config.

### Step 3: Configure Encryption Settings

1. It is strongly advised to configure a recovery key file, as it plays a pivotal role in data
recovery. To do so, update the `encryption` configuration to include a `recoveryKeyUrl`, a local
URL to read the recovery key from. The recovery key file serves as an essential fallback to recover
data should TPM 2.0 automatic decryption fail. If not specified, only the TPM 2.0 device will be
enrolled. Please refer to [the API doc on the `encryption` configuration](../Reference/Host-Configuration/API-Reference/Encryption.md) for additional information on `recoveryKeyUrl`.

1. You can also configure which TPM 2.0 PCRs to seal the encrypted volumes to, by updating the
`pcrs` field. Please refer to [the API doc on the `encryption` configuration](../Reference/Host-Configuration/API-Reference/Encryption.md) for additional information on `pcrs`.

### Step 4: Run Trident to Create Encrypted Volumes

1. [Run `trident install`](./Perform-a-Clean-Install.md) to create the encrypted volume in the target OS. Trident will:

   - Generate a recovery key, or use the provided recovery key.
   - Create a LUKS-encrypted volume on the specified device.
   - Seal the encryption key to the state of the TPM 2.0 device.

1. Once the host boots into the target OS, the encrypted volume will be automatically unlocked, as long as the TPM 2.0 state is as expected. If the boot sequence is somehow corrupted, then the user will be able to manually input the recovery key to unlock the encrypted volume.
