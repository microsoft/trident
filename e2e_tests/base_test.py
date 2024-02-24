import json
import math
import os
import pytest
import yaml

pytestmark = [pytest.mark.base]


class HostStatusSafeLoader(yaml.SafeLoader):
    def accept_image(self, node):
        return self.construct_mapping(node)


def test_connection(connection):
    # Check ssh connection
    result = connection.run("sudo echo 'Successful connection'")
    output = result.stdout.strip()

    assert output == "Successful connection"


def test_partitions(connection, tridentConfiguration):
    # Structure hostConfiguration information
    hostConfiguration = tridentConfiguration["hostConfiguration"]
    expected_partitions = dict()

    for disk_elements in hostConfiguration["storage"]["disks"]:
        for partition in disk_elements["partitions"]:
            expected_partitions[partition["id"]] = partition
            size = int(partition["size"][:-1]) * math.pow(1024, 2)
            size = size * 1024 if partition["size"][-1] == "G" else size
            expected_partitions[partition["id"]]["size"] = size

    # Check partitions type
    result = connection.run("sudo blkid")
    # Expected output example:
    # /dev/sr0: BLOCK_SIZE="2048" UUID="2023-12-16-00-55-13-99" LABEL="TRIDENT_CDROM" TYPE="iso9660"
    # /dev/sda4: LABEL="3e9cecef-5a01-4" UUID="37a7b4fa-87f0-4887-895b-393f46c345a0" TYPE="swap" PARTLABEL="swap" PARTUUID="3e9cecef-5a01-43d6-a1ae-58bf24f42521"
    # /dev/sda2: UUID="04267584-7e18-4612-a649-c71e1811bd82" BLOCK_SIZE="4096" TYPE="ext4" PARTLABEL="root-a" PARTUUID="f1be3a27-36e2-4d4b-b8ec-5b0b5909cbf9"
    # /dev/sda5: LABEL="f3fd8061-ef42-4f" UUID="806ce1d1-44fb-4fb7-8f8d-6f2b21243984" BLOCK_SIZE="4096" TYPE="ext4" PARTLABEL="home" PARTUUID="f3fd8061-ef42-4fa9-8a9a-2903b0bcd1f8"
    # /dev/sda1: SEC_TYPE="msdos" UUID="D920-8BA4" BLOCK_SIZE="512" TYPE="vfat" PARTLABEL="esp" PARTUUID="6fcc7c57-b21c-46e5-bc79-041c7fc53f34"
    # /dev/sda6: LABEL="e87b4510-08b1-4f" UUID="0bb847ed-26cb-496a-b098-1714ca2082a9" BLOCK_SIZE="4096" TYPE="ext4" PARTLABEL="trident" PARTUUID="e87b4510-08b1-4f84-8049-40a69882779b"
    # /dev/sda3: PARTLABEL="root-b" PARTUUID="573fdf4c-9133-4a9f-8cf5-aff7b74d1aeb"

    # Structure output
    partitions_blkid = dict()
    blkid_info = result.stdout.strip().splitlines()

    for partition in blkid_info:
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

    # Check partitions size
    partitions_system_info = dict()
    result = connection.run("lsblk -J")
    lsblk_info = json.loads(result.stdout)
    # Expected output example:
    # {
    #     "blockdevices": [
    #         {
    #             "name": "sda",
    #             "maj:min": "8:0",
    #             "rm": false,
    #             "size": "32G",
    #             "ro": false,
    #             "type": "disk",
    #             "mountpoints": [null],
    #             "children": [
    #                 {
    #                     "name": "sda1",
    #                     "maj:min": "8:1",
    #                     "rm": false,
    #                     "size": "1G",
    #                     "ro": false,
    #                     "type": "part",
    #                     "mountpoints": ["/boot/efi"],
    #                 },
    #                 {
    #                     "name": "sda2",
    #                     "maj:min": "8:2",
    #                     "rm": false,
    #                     "size": "8G",
    #                     "ro": false,
    #                     "type": "part",
    #                     "mountpoints": ["/"],
    #                 },
    #             ],
    #         },
    #         {
    #             "name": "sdb",
    #             "maj:min": "8:16",
    #             "rm": false,
    #             "size": "32G",
    #             "ro": false,
    #             "type": "disk",
    #             "mountpoints": [null],
    #             "children": [
    #                 {
    #                     "name": "sdb1",
    #                     "maj:min": "8:17",
    #                     "rm": false,
    #                     "size": "10M",
    #                     "ro": false,
    #                     "type": "part",
    #                     "mountpoints": [null],
    #                 }
    #             ],
    #         },
    #         {
    #             "name": "sr0",
    #             "maj:min": "11:0",
    #             "rm": true,
    #             "size": "477.9M",
    #             "ro": false,
    #             "type": "rom",
    #             "mountpoints": [null],
    #         },
    #     ]
    # }

    # Gather all partitions from all disks, blockdevices with no children are partitions
    lsblk_partitions = [
        partition
        for block_device in lsblk_info["blockdevices"]
        for partition in (
            block_device["children"] if "children" in block_device else [block_device]
        )
    ]

    # Join lsblk and blkid information to compare with host configuration
    for partition in lsblk_partitions:
        # Update information
        system_name = partition["name"]
        if not system_name in partitions_blkid:
            partitions_blkid[system_name] = dict()
        partitions_blkid[system_name].update(partition)
        # Add information to partitions_system_info which uses PARTLABEL as key
        if "PARTLABEL" in partitions_blkid[system_name]:
            partitions_system_info[partitions_blkid[system_name]["PARTLABEL"]] = (
                partitions_blkid[system_name]
            )
            # Define size of the partitions:
            size = int(partitions_blkid[system_name]["size"][:-1]) * math.pow(1024, 2)
            size = (
                size * 1024
                if partitions_blkid[system_name]["size"][-1] == "G"
                else size
            )
            partitions_system_info[partitions_blkid[system_name]["PARTLABEL"]][
                "size"
            ] = size

    # Check hostStatus
    result = connection.run("/usr/bin/trident get")

    # Structure output
    partitions_host_status = dict()
    host_status_output = result.stdout.strip()

    HostStatusSafeLoader.add_constructor("!image", HostStatusSafeLoader.accept_image)
    host_status = yaml.load(host_status_output, Loader=HostStatusSafeLoader)
    # Gathering all partitions
    partitions_hs_info = [
        partition
        for disk in host_status["storage"]["disks"].values()
        for partition in disk["partitions"]
    ]

    for partition in partitions_hs_info:
        partitions_host_status[partition["id"]] = partition
        partitions_host_status[partition["id"]]["size"] = (
            partition["end"] - partition["start"]
        )

    # Check partitions size and type
    for partition_id in expected_partitions:
        # Partition present
        assert partition_id in partitions_host_status
        assert partition_id in partitions_system_info

        # Partition type
        assert (
            expected_partitions[partition_id]["type"]
            == partitions_host_status[partition_id]["type"]
        )

        # Partition size
        assert (
            expected_partitions[partition_id]["size"]
            == partitions_host_status[partition_id]["size"]
        )
        assert (
            expected_partitions[partition_id]["size"]
            == partitions_system_info[partition_id]["size"]
        )


def test_users(connection, tridentConfiguration):
    # Structure hostConfiguration information
    hostConfiguration = tridentConfiguration["hostConfiguration"]
    expected_users = list()
    expected_groups = dict()

    for user_info in hostConfiguration["osconfig"]["users"]:
        expected_users.append(user_info["name"])
        if "groups" in user_info:
            for group in user_info["groups"]:
                if not group in expected_groups:
                    expected_groups[group] = [user_info["name"]]
                else:
                    expected_groups[group].append(user_info["name"])

    # Check users
    result = connection.run("cat /etc/passwd")
    # Expected output example:
    # root:x:0:0:root:/root:/bin/bash
    # bin:x:1:1:bin:/dev/null:/bin/false
    # daemon:x:6:6:Daemon User:/dev/null:/bin/false
    # messagebus:x:18:18:D-Bus Message Daemon User:/var/run/dbus:/bin/false
    # testing-user:x:1001:1001::/home/testing-user:/bin/bash

    # Structure output
    users_system = set()
    users_info = result.stdout.strip().splitlines()

    for user_info in users_info:
        users_system.add(user_info.split(":")[0])

    for user in expected_users:
        assert user in users_system

    # Check groups
    result = connection.run("cat /etc/group ")
    # Expected output example:
    # root:x:0:
    # bin:x:1:daemon
    # sys:x:2:
    # kmem:x:3:
    # tape:x:4:
    # tty:x:5:

    # Structure output
    users_by_group = dict()
    groups_info = result.stdout.strip().splitlines()

    for group_info in groups_info:
        group_info_elements = group_info.split(":")
        users_by_group[group_info_elements[0]] = set(group_info_elements[-1].split(","))

    for group in expected_groups:
        assert group in users_by_group
        for user in expected_groups[group]:
            assert user in users_by_group[group]
