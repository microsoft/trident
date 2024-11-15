#!/usr/bin/env python3
import argparse
from ssh_utilities import (
    create_ssh_connection,
    run_ssh_command,
)


def get_raid_arrays(connection):
    """
    Get the list of RAID arrays and their devices on the host.
    """
    try:
        # Getting the list of RAID arrays
        result = run_ssh_command(
            connection,
            "mdadm --detail --scan",
            use_sudo=True,
        )
        # Sample output:
        #  ARRAY /dev/md/esp-raid metadata=1.0 name=trident-mos-testimage:esp-raid
        #  UUID=42dd297c:7e0c5a24:6b792c94:238a99f5

        raid_arrays = []
        for line in result.splitlines():
            parts = line.split()
            if len(parts) > 1 and parts[0] == "ARRAY":
                raid_arrays.append(parts[1])

        raid_details = {}

        for raid in raid_arrays:
            # Getting detailed information for each RAID array
            array_result = run_ssh_command(
                connection,
                f"mdadm --detail {raid}",
                use_sudo=True,
            )
            # Sample output:

            # /dev/md/esp-raid:
            #            Version : 1.0
            #      Creation Time : Thu Nov 14 18:17:50 2024
            #         Raid Level : raid1
            #         Array Size : 1048512 (1023.94 MiB 1073.68 MB)
            #      Used Dev Size : 1048512 (1023.94 MiB 1073.68 MB)
            #       Raid Devices : 2
            #      Total Devices : 2
            #        Persistence : Superblock is persistent

            #        Update Time : Thu Nov 14 18:18:49 2024
            #              State : clean
            #     Active Devices : 2
            #    Working Devices : 2
            #     Failed Devices : 0
            #      Spare Devices : 0

            # Consistency Policy : resync

            #               Name : trident-mos-testimage:esp-raid
            #               UUID : 6d52553e:ee0662a3:24761c4b:e3e6885b
            #             Events : 19

            #     Number   Major   Minor   RaidDevice State
            #        0       8        1        0      active sync   /dev/sda1
            #        1       8       17        1      active sync   /dev/sdb1

            details = array_result.splitlines()
            # Extracting devices
            devices = []
            devices_section = False
            for line in details:
                if line.strip().startswith("Number"):
                    devices_section = True
                    continue
                if devices_section and line.strip():
                    parts = line.split()
                    if (
                        len(parts) >= 7
                    ):  # Ensure we have enough parts to avoid index errors
                        devices.append(parts[6])

            raid_details[raid] = devices

        return raid_details

    except Exception as e:
        raise Exception(f"Error getting RAID arrays: {e}")


def fail_raid_array(connection, raid, device):
    """
    Fail a device in a RAID array.
    """
    try:
        run_ssh_command(
            connection,
            f"mdadm --fail {raid} {device}",
            use_sudo=True,
        )
        print(f"Device {device} failed in RAID array {raid}")

    except Exception as e:
        raise Exception(f"Error failing RAID array {raid}: {e}")


def fail_raid_arrays(ip_address, user_name, keys_file_path, disk="/dev/sdb"):
    """
    Fail all devices in RAID arrays that are on the test disk (sdb by default).
    """
    # Set up SSH client
    connection = create_ssh_connection(ip_address, user_name, keys_file_path)

    raid_arrays = get_raid_arrays(connection)
    if raid_arrays:
        for raid, devices in raid_arrays.items():
            for device in devices:
                if device.startswith(disk):
                    # fail the device in the RAID array
                    fail_raid_array(connection, raid, device)

    else:
        print("No RAID arrays found on the host.")


def main():
    # Setting argument_default=argparse.SUPPRESS means that the program will
    # halt attribute creation if no values provided for arg-s
    parser = argparse.ArgumentParser(
        allow_abbrev=True, argument_default=argparse.SUPPRESS
    )
    parser.add_argument(
        "-i",
        "--ip-address",
        type=str,
        help="IP address of the host.",
    )
    parser.add_argument(
        "-u",
        "--user-name",
        type=str,
        help="User name to ssh into the host with.",
    )
    parser.add_argument(
        "-k",
        "--keys-file-path",
        type=str,
        help="Full path to the file containing the host ssh keys.",
    )

    args = parser.parse_args()

    fail_raid_arrays(
        args.ip_address,
        args.user_name,
        args.keys_file_path,
    )


if __name__ == "__main__":
    main()
