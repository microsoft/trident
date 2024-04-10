import json
import math
import os
import pytest
import yaml

pytestmark = [pytest.mark.verity]


class HostStatusSafeLoader(yaml.SafeLoader):
    def accept_image(self, node):
        return self.construct_mapping(node)


def test_verity(connection, tridentConfiguration):
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

    # Collect expected verity info from host config for the later testing usage.
    expected_verity_config = dict()
    host_config = tridentConfiguration["hostConfiguration"]

    for verity in host_config["storage"]["verity"]:
        expected_verity_config[verity["id"]] = verity

    # Check host status.
    res_host_status = connection.run("/usr/bin/trident get")
    output_host_status = res_host_status.stdout.strip()

    HostStatusSafeLoader.add_constructor("!image", HostStatusSafeLoader.accept_image)
    host_status = yaml.load(output_host_status, Loader=HostStatusSafeLoader)
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
    # verity-root:
    #   path: /dev/mapper/root
    #   size: 0
    #   contents: initialized
    # rootDevicePath: /dev/mapper/root

    # Assert if verity info from host config has been involved in host status.
    for verity_id in expected_verity_config:
        assert verity_id in host_status["storage"]["blockDevices"]
        assert "/dev/mapper/root" == host_status["storage"]["rootDevicePath"]

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

    # Assert if veritysetup status.
    assert "VERITY" == veritysetup_status_dict["type"]
    assert "verified" == veritysetup_status_dict["status"]
    assert "readonly" == veritysetup_status_dict["mode"]
