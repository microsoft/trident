import os
import pytest
import yaml
import re
import logging

from base_test import get_raid_name_from_device_name, get_host_status

pytestmark = [pytest.mark.verity]

log = logging.getLogger(__name__)


def test_verity_root(connection, hostConfiguration, tridentCommand, abActiveVolume):
    # Print out result of blkid for asserting verity root device mapper.
    res_blkid = connection.run("sudo blkid")
    # Expected output example:
    # /dev/sdb: PTUUID="a8dbca6f-77a6-485c-8c67-b653758a8928" PTTYPE="gpt"
    # /dev/sr0: BLOCK_SIZE="2048" UUID="2024-04-08-04-36-44-16" LABEL="AZLPROV" TYPE="iso9660"
    # /dev/mapper/root: UUID="aeca4bee-73f3-4ae0-aaa3-57ae0a29ee4b" BLOCK_SIZE="4096" TYPE="ext4"
    # /dev/sda4: UUID="63d564e5-c020-4fff-9b95-5c6553d0b78b" TYPE="DM_verity_hash" PARTLABEL="root-hash" PARTUUID="a9bca9dc-6b36-49be-85e6-46d9e33dbb4a"
    # /dev/sda2: UUID="b00cf2fe-75b4-4aba-8d24-dae00492cf14" BLOCK_SIZE="4096" TYPE="ext4" PARTLABEL="boot" PARTUUID="d33de5a7-f048-4e8f-9334-96d025d8f897"
    # /dev/sda9: UUID="1f99c9cb-13bb-4f33-a20a-557644beb7a7" BLOCK_SIZE="4096" TYPE="ext4" PARTLABEL="run" PARTUUID="da273fd3-d147-4877-a6ea-ab43a827bc1e"
    # /dev/sda7: UUID="a80b9286-31a2-4821-ba1f-0c14cb60c44c" BLOCK_SIZE="1024" TYPE="ext4" PARTLABEL="home" PARTUUID="460ff9f3-e89d-467e-98b5-1b3db0e54a52"
    # /dev/sda5: UUID="059efe02-f439-4fe3-a994-604ec78f047a" BLOCK_SIZE="1024" TYPE="ext4" PARTLABEL="trident" PARTUUID="766418d0-71dc-48d8-919c-7b6ffd88db34"
    # /dev/sda3: UUID="aeca4bee-73f3-4ae0-aaa3-57ae0a29ee4b" BLOCK_SIZE="4096" TYPE="ext4" PARTLABEL="root" PARTUUID="08c2abe7-4d4e-4b8e-b1a2-427e6769a261"
    # /dev/sda1: UUID="74AE-D771" BLOCK_SIZE="512" TYPE="vfat" PARTLABEL="esp" PARTUUID="34beecb1-ae30-4c88-9d8b-df822b927bbc"
    # /dev/sda8: UUID="b056745c-0633-4b3f-a846-923105840c2e" BLOCK_SIZE="4096" TYPE="ext4" PARTLABEL="var" PARTUUID="7f089a07-c487-4a4e-9a18-542f7753ef05"
    # /dev/sda6: UUID="3eaa3f16-6e33-415d-ade8-7cb296f61a42" BLOCK_SIZE="1024" TYPE="ext4" PARTLABEL="trident-overlay" PARTUUID="3407f514-2b2f-4016-8bdb-2f56285d497b"

    # Structure blkid output.
    output_blkid = res_blkid.stdout.strip().splitlines()

    part_path_set = set()
    for part in output_blkid:
        # Extract partition path (example: /dev/sda1).
        part_path = part.split(": ")
        part_path_set.add(part_path[0])

    # Assert if /dev/mapper/root has been generated properly.
    assert "/dev/mapper/root" in part_path_set

    partitions_blkid = dict()
    for partition in output_blkid:
        partition_dict = dict()
        # Extract partition's name (ex: sda1)
        name_info = partition.split(": ")
        name = name_info[0].split("/")[-1]
        # By line structure output into a dictionary with the partition information
        for info in name_info[1].split():
            field_value = info.split("=")
            if len(field_value) == 2:
                partition_dict[field_value[0]] = field_value[1].replace('"', "")
        # Adding information to a dictionary for each partition
        partitions_blkid[name] = partition_dict

    # Collect expected verity info from host config for the later testing usage.
    expected_verity_config = dict()

    items = hostConfiguration["storage"].get("verity", [])

    for verity in items:
        expected_verity_config[verity["name"]] = verity

    # Collect veritysetup status output.
    veritysetup_status = connection.run("sudo veritysetup status root")
    # veritysetup status expected output example:
    # /dev/mapper/root is active and is in use.
    #   type:        VERITY
    #   status:      verified
    #   hash type:   1
    #   data block:  4096
    #   hash block:  4096
    #   hash name:   sha256
    #   salt:        95c671631e5202431ead38146e1af8342100ff03bc2a89f2590dcb3454cc6e31
    #   data device: /dev/sda3
    #   size:        1377128 sectors
    #   mode:        readonly
    #   hash device: /dev/sda4
    #   hash offset: 8 sectors
    #   root hash:   a8c34ed685f365352231db21aa36ff23bf8b658e001afa8e498f57d1755e9a19
    #   flags:       panic_on_corruption

    # Structure veritysetup status output.
    output_veritysetup_status = veritysetup_status.stdout.strip().splitlines()

    # Assert if verity target is active.
    assert "/dev/mapper/root is active and is in use." == output_veritysetup_status[0]

    # Organize dict for the veritysetup output for the following assert.
    veritysetup_status_dict = dict()
    for status in output_veritysetup_status[1:]:
        key, value = [
            item.strip() for item in status.split(":", 1)
        ]  # Strip spaces from both key and value
        if key and value:
            veritysetup_status_dict[key] = value
        else:
            raise ValueError(
                f"Invalid key or value from status: key='{key}', value='{value}'"
            )

    # Validate key properties of the veritysetup output to ensure the verity
    # device is configured correctly.
    assert "VERITY" == veritysetup_status_dict["type"]
    assert "verified" == veritysetup_status_dict["status"]
    assert "readonly" == veritysetup_status_dict["mode"]

    # Check Host Status.
    host_status = get_host_status(connection, tridentCommand)

    # Host status expected output example:
    # root:
    #   path: /dev/disk/by-partuuid/f69514c7-d20a-42fd-8c4e-49df24d2ce40
    #   size: 8589934592
    #   contents: !image
    #     sha256: 764292ca5261af4d68217381d5e2520f453ca22d2af38c081dfc93aeda075d0b
    #     length: 705090048
    #     url: http://10.1.6.1:36439/files/verity_root.rawzst
    # root-hash:
    #   path: /dev/disk/by-partuuid/290ddc62-c339-457c-989d-5551153fcb9c
    #   size: 1073741824
    #   contents: !image
    #     sha256: b63a60a5c6d172cf11d0aec785f50414d2d46206a64e95639804b85c8fa0f3e5
    #     length: 25321984
    #     url: http://10.1.6.1:36439/files/verity_roothash.rawzst
    # verity-0:
    #   path: /dev/mapper/root
    #   size: 0
    #   contents: initialized

    # Assert verity data device and hash device. Refer to logic from base test
    # to extract the ID of the mount point with path "/".
    root_mount_id = None
    # Look for it in filesystems
    for fs in host_status["spec"]["storage"]["filesystems"]:
        mp = fs.get("mountPoint")
        if not mp:
            continue
        if mp["path"] == "/":
            root_mount_id = fs.get("deviceId")
            break

    # If no mount point with path / found, raise an exception
    if root_mount_id is None:
        raise Exception("Root mount point not found")

    # If root_mount_id is still None, look in verity
    verity_device_name = None
    data_device_id = None
    hash_device_id = None
    items = host_status["spec"]["storage"].get("verity", [])
    for verity_dev in items:
        if verity_dev.get("id") == root_mount_id:
            verity_device_name = verity_dev.get("name")
            data_device_id = verity_dev.get("dataDeviceId")
            hash_device_id = verity_dev.get("hashDeviceId")
            break

    # If root is not a verity device, no more testing to do here
    if verity_device_name is None or hash_device_id is None:
        raise Exception("No verity configuration found for the provided root mount ID")

    if "abUpdate" in host_status["spec"]["storage"] and abActiveVolume is not None:
        active_data_id, active_hash_id = None, None
        # Identify block devices we expect to be in use, given the value of abActiveVolume.
        for volume_pair in host_status["spec"]["storage"]["abUpdate"]["volumePairs"]:
            if volume_pair["id"] == data_device_id:
                if abActiveVolume == "volume-a":
                    active_data_id = volume_pair["volumeAId"]
                else:
                    active_data_id = volume_pair["volumeBId"]

            if volume_pair["id"] == hash_device_id:
                if abActiveVolume == "volume-a":
                    active_hash_id = volume_pair["volumeAId"]
                else:
                    active_hash_id = volume_pair["volumeBId"]
        assert active_data_id is not None and active_hash_id is not None

        # Run and process `veritysetup status`
        data_block_device, hash_block_device = get_data_hash_from_veritysetup(
            connection, verity_device_name
        )

        # Check if data_block_device, hash_block_device correspond to partitions or RAID arrays
        data_is_raid = get_raid_name_from_device_name(connection, data_block_device)
        hash_is_raid = get_raid_name_from_device_name(connection, hash_block_device)
        # Assert that both data_is_raid are either both None or both not None
        assert (data_is_raid is None) == (
            hash_is_raid is None
        ), f"Assertion failed: data_is_raid={data_is_raid}, hash_is_raid={hash_is_raid}"

        # If get_raid_name_from_device_name() returned a non-null value, block device is a RAID
        # array.
        if data_is_raid:
            # Convert /dev/md/root-a into root-a; /dev/sda1 into sda1
            extracted_data_block_device = data_is_raid.split("/")[-1]
            extracted_hash_block_device = hash_is_raid.split("/")[-1]

            assert active_data_id == extracted_data_block_device
            assert active_hash_id == extracted_hash_block_device
        else:
            # If get_raid_name_from_device_name() returned None, block device is a partition.
            # NOTE: This check assumes that PARTLABEL in blkid is same as device ID in Host Status.
            extracted_data_block_device = data_block_device.split("/")[-1]
            extracted_hash_block_device = hash_block_device.split("/")[-1]

            assert (
                extracted_data_block_device in partitions_blkid
                and extracted_hash_block_device in partitions_blkid
            )
            assert (
                partitions_blkid[extracted_data_block_device]["PARTLABEL"]
                == active_data_id
            )
            assert (
                partitions_blkid[extracted_hash_block_device]["PARTLABEL"]
                == active_hash_id
            )
    else:
        # Retrieve data device and hash device from veritysetup status.
        data_block_device = veritysetup_status_dict["data device"]
        hash_block_device = veritysetup_status_dict["hash device"]

        data_is_raid = get_raid_name_from_device_name(connection, data_block_device)
        hash_is_raid = get_raid_name_from_device_name(connection, hash_block_device)
        assert (data_is_raid is None) == (
            hash_is_raid is None
        ), f"Assertion failed: data_is_raid={data_is_raid}, hash_is_raid={hash_is_raid}"

        if data_is_raid:
            # Convert for example /dev/md/root into root.
            extracted_data_block_device = os.path.basename(data_is_raid)
            extracted_hash_block_device = os.path.basename(hash_is_raid)

            assert data_device_id == extracted_data_block_device
            assert hash_device_id == extracted_hash_block_device
        else:
            extracted_data_block_device = data_block_device.split("/")[-1]
            extracted_hash_block_device = hash_block_device.split("/")[-1]

            assert (
                extracted_data_block_device in partitions_blkid
                and extracted_hash_block_device in partitions_blkid
            )


# Runs 'verity setup' and returns the block device paths of root data device and hash device. E.g.
# with the sample output below, func returns a tuple  (/dev/sda3, /dev/sda4).
def get_data_hash_from_veritysetup(connection, device_name):
    # Expected output example:
    # /dev/mapper/root is active and is in use.
    # type:        VERITY
    # status:      verified
    # hash type:   1
    # data block:  4096
    # hash block:  4096
    # hash name:   sha256
    # salt:        1edfc828a4d3116dc42a8457489db9e9024657382c7e6e27fb16a23b8ad68e56
    # data device: /dev/sda3
    # size:        1377048 sectors
    # mode:        readonly
    # hash device: /dev/sda4
    # hash offset: 8 sectors
    # root hash:   d446a67f7521af5bbeb2144b85b0859780d75960475e3dde291236054c59d97a
    # flags:       panic_on_corruption
    # OR
    # /dev/mapper/root is active and is in use.
    # type:        VERITY
    # status:      verified
    # hash type:   1
    # data block:  4096
    # hash block:  4096
    # hash name:   sha256
    # salt:        d16ba3427abe5c98dcb320d484672cdbce159476ada1831bfdf05be0a7072a50
    # data device: /dev/md126
    # size:        1377024 sectors
    # mode:        readonly
    # hash device: /dev/md127
    # hash offset: 8 sectors
    # root hash:   a35ecce908f6a29fc0dbd56f6cd2216c9c9c883d503252b92f97092b540df9d7
    # flags:       panic_on_corruption

    try:
        # Run 'veritysetup status' command
        command_output = connection.run(f"sudo veritysetup status {device_name}")
        status_output = command_output.stdout.strip()

        # Parse the output
        data_device_match = re.search(r"data device: (/dev/\S+)", status_output)
        hash_device_match = re.search(r"hash device: (/dev/\S+)", status_output)

        if data_device_match and hash_device_match:
            root_data_device = data_device_match.group(1)
            root_hash_device = hash_device_match.group(1)
            return (root_data_device, root_hash_device)
    except Exception as e:
        raise Exception(f"Unexpected error") from e

    return None
