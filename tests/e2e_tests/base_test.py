import fabric
import json
import math
import pytest
import re
import yaml
from enum import Enum

pytestmark = [pytest.mark.base]


# Size units
class SizeUnit(Enum):
    B = 1
    K = math.pow(1024, 1)
    M = math.pow(1024, 2)
    G = math.pow(1024, 3)
    T = math.pow(1024, 4)
    P = math.pow(1024, 5)


def test_connection(connection):
    # Check ssh connection
    result = connection.run("sudo echo 'Successful connection'")
    output = result.stdout.strip()

    assert output == "Successful connection"


def test_partitions(connection, hostConfiguration, tridentCommand, abActiveVolume):
    # Structure hostConfiguration information
    expected_partitions = dict()

    for disk_elements in hostConfiguration["storage"]["disks"]:
        for partition in disk_elements["partitions"]:
            # Extract size in bytes
            size_number = partition["size"][:-1]
            unit = partition["size"][-1] if partition["size"][-1].isalpha() else "B"
            size = float(size_number) * SizeUnit[unit].value
            # Update the expected partitions dictionary
            expected_partitions[partition["id"]] = partition
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
    result = connection.run("lsblk -J -b")
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

    # Join lsblk and blkid information to compare with Host Configuration
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

    # Check Host Status
    host_status = get_host_status(connection, tridentCommand)

    # Check that servicing state is as expected
    assert host_status["servicingState"] == "provisioned"

    # Check partitions size and type
    for partition_id in expected_partitions:
        # Partition present
        assert partition_id in host_status["partitionPaths"]
        assert partition_id in partitions_system_info

    # Fetch path of block device mounted at /
    root_device_path_canonicalized = get_root_device_path_from_mount(connection)

    # Perform checks for A/B update only
    if "abUpdate" in host_status["spec"]["storage"] and abActiveVolume is not None:
        # Extract the ID of the mount point with path "/"
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

        print(f"Root mount point ID: {root_mount_id}")

        verity_device_name = None
        verity_data_device_id = None
        for verity_dev in host_status["spec"]["storage"].get("verity", []):
            print("Inspecting verity device:", verity_dev)
            if verity_dev.get("id") == root_mount_id:
                print(f"Found verity device with matching ID '{root_mount_id}'")
                verity_device_name = verity_dev.get("name")
                verity_data_device_id = verity_dev.get("dataDeviceId")
                break

        print(f"Verity device name: {verity_device_name}")
        print(f"Verity data device ID: {verity_data_device_id}")

        # Find the ID of the AB volume pair. If verity_data_device_id is set,
        # the root filesystem is on a verity device. This device MUST be on an A/B
        # volume pair. The volume pair ID is the ID of the verity device.
        # If verity_data_device_id is not set, the root filesystem is on a non-verity
        # device. In this case, the ID of the AB volume pair is the device the filesystem is on.
        ab_volume_id = (
            verity_data_device_id
            if verity_data_device_id is not None
            else root_mount_id
        )

        print(f"Root A/B volume ID: {ab_volume_id}")

        # Check the block device mounted at /. For verity devices, root and
        # root-hash A/B volume pairs are tested in verity_test.py. In this
        # test, we focus on configurations where abUpdate is enabled, ensuring
        # that root is part of an A/B volume pair. This test identifies the
        # active volume ID for the root mount point.
        if verity_device_name is None:
            active_volume_id = None
            for volume_pair in host_status["spec"]["storage"]["abUpdate"][
                "volumePairs"
            ]:
                if volume_pair["id"] == ab_volume_id:
                    print(f"Found volume pair: {ab_volume_id}")
                    if abActiveVolume == "volume-a":
                        active_volume_id = volume_pair["volumeAId"]
                    else:
                        active_volume_id = volume_pair["volumeBId"]
                    print(f"Active volume ID: {active_volume_id}")
                    break

            assert active_volume_id is not None

            active_volume_is_partition = is_partition(host_status, active_volume_id)
            active_volume_is_raid = is_raid(host_status, active_volume_id)
            # active_volume_id should be either a partition or a software RAID array
            assert (active_volume_is_partition and not active_volume_is_raid) or (
                not active_volume_is_partition and active_volume_is_raid
            )

            # 1. If active_volume_id is a partition, get full PARTUUID based on blkid output and create
            # root_device_path, non-canonicalized
            root_device_path = None

            if active_volume_is_partition:
                for partition_name, partition_info in partitions_blkid.items():
                    if partition_name == root_device_path_canonicalized.split("/")[-1]:
                        root_device_path = (
                            f"/dev/disk/by-partuuid/{partition_info['PARTUUID']}"
                        )
            # 2. If active_volume_id is a software RAID array, run 'ls -l /dev/md' to fetch full name
            # of RAID array mounted at root /
            elif active_volume_is_raid:
                root_device_path = get_raid_name_from_device_name(
                    connection, root_device_path_canonicalized
                )

            # Iterate through block devices and confirm that path of active volume corresponds to
            # non-canonicalized root device path
            for block_device_id, block_device_path in host_status[
                "partitionPaths"
            ].items():
                if block_device_id == active_volume_id:
                    assert block_device_path == root_device_path

            # Verify abActiveVolume
            assert host_status["abActiveVolume"] == abActiveVolume


# Returns true if block device with block_device_id is a partition; otherwise, returns false
def is_partition(host_status, block_device_id):
    for disk in host_status["spec"]["storage"]["disks"]:
        for partition in disk.get("partitions", []):
            if partition["id"] == block_device_id:
                return True
    return False


# Returns true if block device with target_id is a software RAID array; otherwise, returns false
def is_raid(host_status, block_device_id):
    for raid in host_status["spec"]["storage"].get("raid", {}).get("software", []):
        if raid["id"] == block_device_id:
            return True
    return False


def get_host_status(connection: fabric.Connection, tridentCommand: str) -> dict:
    """
    Get the Host Status by running `trident get` on the given connection,
    and return the parsed YAML output.
    """

    cmd = f"{tridentCommand} get"
    result = connection.run(cmd)

    # Structure output
    output = result.stdout.strip()

    yaml.add_multi_constructor(
        "!", lambda loader, _, node: loader.construct_mapping(node)
    )
    return yaml.load(output, Loader=yaml.FullLoader)


# Runs 'mount' and returns the name of the block device mounted at root /
def get_root_device_path_from_mount(connection):
    # Expected output example:
    # /dev/sda3 on / type ext4 (rw,relatime)
    # devtmpfs on /dev type devtmpfs (rw,nosuid,size=4096k,nr_inodes=721913,mode=755)
    # tmpfs on /dev/shm type tmpfs (rw,nosuid,nodev)
    # devpts on /dev/pts type devpts (rw,nosuid,noexec,relatime,gid=5,mode=620,ptmxmode=000)
    # sysfs on /sys type sysfs (rw,nosuid,nodev,noexec,relatime)
    # securityfs on /sys/kernel/security type securityfs (rw,nosuid,nodev,noexec,relatime)
    # tmpfs on /sys/fs/cgroup type tmpfs (ro,nosuid,nodev,noexec,size=4096k,nr_inodes=1024,mode=755)
    # cgroup on /sys/fs/cgroup/systemd type cgroup (rw,nosuid,nodev,noexec,relatime,xattr,release_agent=/usr/lib/systemd/systemd-cgroups-agent,name=systemd)
    # cgroup on /sys/fs/cgroup/freezer type cgroup (rw,nosuid,nodev,noexec,relatime,freezer)
    # cgroup on /sys/fs/cgroup/net_cls,net_prio type cgroup (rw,nosuid,nodev,noexec,relatime,net_cls,net_prio)
    # cgroup on /sys/fs/cgroup/hugetlb type cgroup (rw,nosuid,nodev,noexec,relatime,hugetlb)
    # cgroup on /sys/fs/cgroup/cpu,cpuacct type cgroup (rw,nosuid,nodev,noexec,relatime,cpu,cpuacct)
    # cgroup on /sys/fs/cgroup/memory type cgroup (rw,nosuid,nodev,noexec,relatime,memory)
    # cgroup on /sys/fs/cgroup/perf_event type cgroup (rw,nosuid,nodev,noexec,relatime,perf_event)
    # cgroup on /sys/fs/cgroup/cpuset type cgroup (rw,nosuid,nodev,noexec,relatime,cpuset)
    # cgroup on /sys/fs/cgroup/devices type cgroup (rw,nosuid,nodev,noexec,relatime,devices)
    # cgroup on /sys/fs/cgroup/blkio type cgroup (rw,nosuid,nodev,noexec,relatime,blkio)
    # cgroup on /sys/fs/cgroup/pids type cgroup (rw,nosuid,nodev,noexec,relatime,pids)
    # cgroup on /sys/fs/cgroup/rdma type cgroup (rw,nosuid,nodev,noexec,relatime,rdma)
    # pstore on /sys/fs/pstore type pstore (rw,nosuid,nodev,noexec,relatime)
    # efivarfs on /sys/firmware/efi/efivars type efivarfs (rw,nosuid,nodev,noexec,relatime)
    # bpf on /sys/fs/bpf type bpf (rw,nosuid,nodev,noexec,relatime,mode=700)
    # proc on /proc type proc (rw,nosuid,nodev,noexec,relatime)
    # tmpfs on /run type tmpfs (rw,nosuid,nodev,size=1159040k,nr_inodes=819200,mode=755)
    # systemd-1 on /proc/sys/fs/binfmt_misc type autofs (rw,relatime,fd=27,pgrp=1,timeout=0,minproto=5,maxproto=5,direct,pipe_ino=17284)
    # hugetlbfs on /dev/hugepages type hugetlbfs (rw,nosuid,nodev,relatime,pagesize=2M)
    # mqueue on /dev/mqueue type mqueue (rw,nosuid,nodev,noexec,relatime)
    # debugfs on /sys/kernel/debug type debugfs (rw,nosuid,nodev,noexec,relatime)
    # tracefs on /sys/kernel/tracing type tracefs (rw,nosuid,nodev,noexec,relatime)
    # fusectl on /sys/fs/fuse/connections type fusectl (rw,nosuid,nodev,noexec,relatime)
    # configfs on /sys/kernel/config type configfs (rw,nosuid,nodev,noexec,relatime)
    # tmpfs on /tmp type tmpfs (rw,nosuid,nodev,size=2897596k,nr_inodes=1048576)
    # /dev/sda5 on /home type ext4 (rw,relatime)
    # /dev/sda1 on /boot/efi type vfat (rw,relatime,fmask=0077,dmask=0077,codepage=437,iocharset=ascii,shortname=mixed,errors=remount-ro)
    # /dev/sda6 on /var/lib/trident type ext4 (rw,relatime)
    # tmpfs on /run/user/1001 type tmpfs (rw,nosuid,nodev,relatime,size=579516k,nr_inodes=144879,mode=700,uid=1001,gid=1001)
    try:
        mount_result = connection.run("mount")
        mount_info = mount_result.stdout.strip().splitlines()

        partitions_mount_info = dict()
        for line in mount_info:
            # Assuming the format is 'device on mount_point type fs_type (options)'
            parts = line.split()
            if len(parts) >= 3:
                device_name = parts[0]
                mount_point = parts[2]
                fs_type = parts[4] if len(parts) > 4 else "unknown"
                partitions_mount_info[device_name] = {
                    "mount_point": mount_point,
                    "fs_type": fs_type,
                }

        # Find name of block device mounted at root /
        for device_name, info in partitions_mount_info.items():
            if info["mount_point"] == "/":
                return device_name
    except Exception as e:
        print(f"An error occurred: {e}")
        return None

    return None


# Runs 'ls -l /dev/md' and returns the name of RAID array that corresponds to device_name. E.g. if
# device_name is /dev/md127 then func returns /dev/md/root-a.
def get_raid_name_from_device_name(connection, device_name):
    # Expected output example:
    # lrwxrwxrwx 1 root root 8 Apr  1 22:42 home -> ../md124
    # lrwxrwxrwx 1 root root 8 Apr  1 22:42 root-a -> ../md127
    # lrwxrwxrwx 1 root root 8 Apr  1 22:42 root-b -> ../md125
    # lrwxrwxrwx 1 root root 8 Apr  1 22:42 trident -> ../md126
    try:
        md_device_number = (
            device_name.split("/")[-1] if "/" in device_name else device_name
        )

        # Execute command to get RAID names and corresponding devices
        command_output = connection.run("ls -l /dev/md || true", warn=True)
        raid_output = command_output.stdout.strip().splitlines()

        # If there is no output, return None
        if not raid_output or "No such file or directory" in command_output.stderr:
            print("'/dev/md' directory does not exist or is empty")
            return None

        for line in raid_output:
            if md_device_number in line:
                # Extract the RAID name
                match = re.search(
                    r"(\S+)\s+-> \.\./" + re.escape(md_device_number), line
                )
                if match:
                    return f"/dev/md/{match.group(1)}"

        return None

    except Exception as e:
        print(f"An error occurred: {e}")
        return None


def test_users(connection, hostConfiguration):
    # Structure hostConfiguration information
    expected_users = list()
    expected_groups = dict()

    for user_info in hostConfiguration["os"]["users"]:
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


def test_uefi_fallback(connection, hostConfiguration):
    mode = "rollback"  # Default mode if not set
    if "uefiFallback" in hostConfiguration:
        mode = hostConfiguration["os"]["uefiFallback"]

    if mode not in ["none", "rollback", "rollforward"]:
        raise Exception(f"Unknown uefiFallback mode: {mode}")

    if mode == "none":
        # Check that /efi/boot/EFI/BOOT is empty
        connection.run("sudo find /efi/boot/EFI/BOOT/* && exit 1 || exit 0")
        return

    # Check that /efi/boot/EFI/BOOT/* is same as /efi/azl/EFI/<CURRENTBOOT>/*
    result = connection.run("sudo efibootmgr | grep BootCurrent | awk '{print $2}')")
    current_boot_entry = result.stdout.strip().splitlines()
    result = connection.run(
        f"sudo efibootmgr | grep Boot{current_boot_entry} | awk '{{print $2}}'"
    )
    current_boot_name = result.stdout.strip().splitlines()
    connection.run(
        f"sudo diff /efi/boot/EFI/BOOT/* /efi/azl/EFI/{current_boot_name}/* && exit 1 || exit 0"
    )
