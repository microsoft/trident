import json
import typing
import fabric
import pytest

pytestmark = [pytest.mark.encryption]


def get_filesystem(hostConfiguration: dict, fsId: str) -> typing.Optional[dict]:
    """
    Get the filesystem for the given filesystem ID in the Trident
    configuration, or None if no such filesystem exists.
    """

    for fs in hostConfiguration["storage"]["filesystems"]:
        if fs["deviceId"] == fsId:
            return fs

    return None


def get_swap(hostConfiguration: dict, devId: str) -> typing.Optional[dict]:
    """Gets the swap device associated with the provided device id, if any."""

    for swap in hostConfiguration["storage"].get("swap", []):
        if isinstance(swap, str) and swap == devId:
            return {"deviceId": devId}
        elif isinstance(swap, dict) and swap["deviceId"] == devId:
            return swap

    return None


def get_active_swaps(connection: fabric.Connection) -> typing.Set[str]:
    active = sudo(
        connection,
        "swapon --show=NAME --raw --bytes --noheadings | xargs -I @ readlink -f @",
    )

    return set(active.splitlines())


def get_child_ab_update_volume_pair(
    hostConfiguration: dict, cryptId: str
) -> typing.Tuple[typing.Optional[dict], bool]:
    if "abUpdate" not in hostConfiguration["storage"]:
        return None, False

    for abUpdateVolumePair in hostConfiguration["storage"]["abUpdate"]["volumePairs"]:
        if abUpdateVolumePair["volumeAId"] == cryptId:
            return abUpdateVolumePair, True

        if abUpdateVolumePair["volumeBId"] == cryptId:
            return abUpdateVolumePair, False

    return None, False


def get_raid_software_array_name(
    hostConfiguration: dict, aId: str
) -> typing.Optional[str]:
    """
    Get the name of the RAID software array with the given ID in the
    Trident configuration, or None if no such array exists.
    """

    for a in hostConfiguration["storage"]["raid"]["software"]:
        if a["id"] == aId:
            return a["name"]

    return None


def get_disk_partition(hostConfiguration: dict, pId: str) -> typing.Optional[dict]:
    """
    Check if a disk partition with the given ID exists in the Trident
    configuration.
    """

    for d in hostConfiguration["storage"]["disks"]:
        for p in d["partitions"]:
            if p["id"] == pId:
                return p

    return None


def read_dict_from_lines(lines: list[str]) -> dict:
    """
    Read a dictionary from a list of lines in the format "key: value".
    """

    d = {}
    for line in lines:
        k, v = line.split(":", 1)
        d[k.strip()] = v.strip()

    return d


def read_table_from_stdout(stdout: str) -> list[dict]:
    """
    Read a table from the given stdout string. The first line is expected
    to contain the column headers, and the following lines are expected to
    contain the rows. The columns are separated by whitespace.
    """

    lines = stdout.splitlines()
    header = [c.strip() for c in lines[0].split()]
    rows = [[c.strip() for c in line.split()] for line in lines[1:]]
    return [dict(zip(header, r)) for r in rows]


def sudo(connection: fabric.Connection, cmd: str) -> str:
    """
    Run the given command with sudo on the given connection and return the
    stripped stdout.
    """
    res = connection.run(f"sudo {cmd}")
    return res.stdout.strip()


def get_blkid_output(connection: fabric.Connection) -> dict:
    """
    Get the output of `blkid --output export` and return a dictionary
    mapping device names to their properties.

    Example output:

            # blkid --output export
            DEVNAME=/dev/md127
            UUID=475f0351-4bb7-49bb-b9af-1f53f94b91cb
            TYPE=crypto_LUKS

            DEVNAME=/dev/sr0
            BLOCK_SIZE=2048
            UUID=2024-10-30-22-05-47-00
            LABEL=CDROM
            TYPE=iso9660
            ...
    """
    cmd = "blkid --output export"
    stdout = sudo(connection, cmd)

    devs: dict[str, dict] = {}
    name = None
    for line in stdout.splitlines():
        if line == "":
            continue

        k, v = line.split("=", 1)
        if k == "DEVNAME":
            name = v
            devs[name] = {}
        elif name is not None:
            devs[name][k] = v
        else:
            raise ValueError(f"Unexpected line: {line}")

    return devs


def check_exists(connection: fabric.Connection, path: str) -> None:
    """
    Check if the given path exists by running `ls` on it.
    """
    cmd = f"ls {path}"
    _ = sudo(connection, cmd)


def check_cryptsetup_status(
    connection: fabric.Connection, name: str, isInUse: bool
) -> dict:
    """
    Check the output of `cryptsetup status` for the given device name.

    Example output:

        # cryptsetup status web
        /dev/mapper/web is active and is in use.
        type:    n/a
        cipher:  aes-xts-plain64
        keysize: 512 bits
        key location: keyring
        device:  /dev/md127
        sector size:  512
        offset:  16384 sectors
        size:    2080640 sectors
        mode:    read/write
    """

    cmd = f"cryptsetup status {name}"
    stdout = sudo(connection, cmd)
    lines = stdout.splitlines()

    # LUKS2-encrypted volumes are always opened and therefore always
    # active according to cryptsetup. When a volume is a member of an AB
    # update pair, but is inactive, it won't be mounted, and so cryptsetup
    # will not report it as being used.
    if isInUse:
        expected_first_line = f"/dev/mapper/{name} is active and is in use."
        assert (
            lines[0] == expected_first_line
        ), f"Expected first line to be {expected_first_line!r}, got {lines[0]!r}"
    else:
        expected_first_line = f"/dev/mapper/{name} is active."
        assert (
            lines[0] == expected_first_line
        ), f"Expected first line to be {expected_first_line!r}, got {lines[0]!r}"

    status = read_dict_from_lines(lines[1:])

    expected_cipher = "aes-xts-plain64"
    assert (
        status["cipher"] == expected_cipher
    ), f"Expected cipher to be {expected_cipher!r}, got {status['cipher']!r}"

    expected_keysize = "512 bits"
    assert (
        status["keysize"] == expected_keysize
    ), f"Expected keysize to be {expected_keysize!r}, got {status['keysize']!r}"

    return status


def check_dmsetup_info(connection: fabric.Connection, name: str, swap: bool) -> None:
    """
    Check the output of `dmsetup info` for the given device name.

    Example output:

        # dmsetup info /dev/mapper/web
        Name:              web
        State:             ACTIVE
        Read Ahead:        256
        Tables present:    LIVE
        Open count:        0
        Event number:      0
        Major, minor:      254, 0
        Number of targets: 1
        UUID: CRYPT-LUKS2-475f03514bb749bbb9af1f53f94b91cb-web
    """
    cmd = f"dmsetup info {name}"
    stdout = sudo(connection, cmd)
    info = read_dict_from_lines(stdout.splitlines())

    assert "Name" in info, f"Expected Name to be in {info!r}"
    assert info["Name"] == name, f"Expected Name to be {name!r}, got {info['Name']!r}"

    expected_state = "ACTIVE"
    assert (
        info["State"] == expected_state
    ), f"Expected State to be {expected_state!r}, got {info['State']!r}"

    expected_tables_present = "LIVE"
    assert (
        info["Tables present"] == expected_tables_present
    ), f"Expected Tables present to be {expected_tables_present!r}, got {info['Tables present']!r}"

    crypt_kind = "PLAIN" if swap else "LUKS2"
    expected_uuid_prefix = f"CRYPT-{crypt_kind}-"
    assert info["UUID"].startswith(
        expected_uuid_prefix
    ), f"Expected UUID to start with {expected_uuid_prefix!r}, got {info['UUID']!r}"

    expected_uuid_suffix = f"-{name}"
    assert info["UUID"].endswith(
        f"-{name}"
    ), f"Expected UUID to end with {expected_uuid_suffix!r}, got {info['UUID']!r}"


def check_findmnt(
    connection: fabric.Connection, target: str, source: str, isActive: bool
) -> None:
    """
    Check the output of `findmnt` for the given path and encrypted device.

    Example output:

        # findmnt /mnt/web
        TARGET SOURCE FSTYPE OPTIONS
        /mnt/web /dev/mapper/web ext4 rw,relatime
    """
    cmd = f"findmnt {target}"
    stdout = sudo(connection, cmd)
    table = read_table_from_stdout(stdout)

    assert (
        table[0]["TARGET"] == target
    ), f"Expected TARGET to be {target!r}, got {table[0]['TARGET']!r}"

    expected_fstype = "ext4"

    if isActive:
        assert (
            table[0]["SOURCE"] == source
        ), f"Expected SOURCE to be {source!r} when it is active, got {table[0]['SOURCE']!r}"
        assert (
            table[0]["FSTYPE"] == expected_fstype
        ), f"Expected FSTYPE to be {expected_fstype!r} when {source!r} is active, got {table[0]['FSTYPE']!r}"
    else:
        assert (
            table[0]["SOURCE"] != source
        ), f"Expected SOURCE to be different from {source!r} when it is not active."
        assert (
            table[0]["FSTYPE"] == expected_fstype
        ), f"Expected FSTYPE to be {expected_fstype!r} even when {source!r} is not active, got {table[0]['FSTYPE']!r}"

    assert len(table) == 1, f"Expected one row, got {len(table)}. Table: {table}"


def get_block_dev_path_by_partlabel(
    blockDevs: dict, label: str
) -> typing.Optional[str]:
    """
    Get the device path for the device with the given PARTLABEL, or None
    if no such device exists.
    """

    for devId, dev in blockDevs.items():
        if "PARTLABEL" in dev and dev["PARTLABEL"] == label:
            return devId

    return None


def check_crypsetup_luks_dump(connection: fabric.Connection, cryptDevPath: str) -> None:
    """
    Check the output of `cryptsetup luksDump --dump-json-metadata` for the
    given device path.

    Example output:

        {
            "keyslots": {
                "2": {
                "type": "luks2",
                "key_size": 64,
                "af": {
                    "type": "luks1",
                    "stripes": 4000,
                    "hash": "sha512"
                },
                "area": {
                    "type": "raw",
                    "offset": "548864",
                    "size": "258048",
                    "encryption": "aes-xts-plain64",
                    "key_size": 64
                },
                "kdf": {
                    "type": "pbkdf2",
                    "hash": "sha512",
                    "iterations": 1000,
                    "salt": "ckq4BDEkrmGTcFjcY9dI1e+/iyn1ksgI9RvGNiS52Rs="
                }
                }
            },
            "tokens": {
                "1": {
                "type": "systemd-tpm2",
                "keyslots": [
                    "2"
                ],
                "tpm2-blob": "AJ4AIPe69RFTvAlGkBaLZ9XFfiPhKXUA7FEKFZF5grqoot9tABCPMeDTUIP9JaS/0A6yUaW9y/JjVo0gKnoDALPibV20RPhwHAFg6ycQdQX1sbyhIa/+CmzfMvHOM7+cYZXiq6O/ZIF9hWKMtRUg47Q8C8ok0dyxFWow8wQy7woH0p94pUeCGBmgq34smc3aCUdnjl/TQLDvsgmLlpJHnwBOAAgACwAAABIAIDzHev7RjwqkxM/9b4dCkH0O2Kd96RwB2CLhE2PMOkSRABAAIEx0aFr/1AgrYBoB6qrLsHkXvkEuPOWd5Ns2AQx0uHoh",
                "tpm2-pcrs": [],
                "tpm2-policy-hash": "3cc77afed18f0aa4c4cffd6f8742907d0ed8a77de91c01d822e11363cc3a4491",
                "tpm2-pin": false,
                "tpm2_pcrlock": true,
                "tpm2_srk": "gQAAAQAiAAupLdVcuox9yfRZaxtvQ8X/Dj/VK4OEY2X42DKM+xBzfAAAAAEAWgAjAAsAAwRyAAAABgCAAEMAEAADABAAIAmK45y/lGMhOdONab4wzGT43Yt3oZDCSATydlLlP5gTACBb1qkPGKbv248ZsEvDhA4zdEnOIjkcFD/hxtff5IzRgQ=="
                }
            },
            "segments": {
                "0": {
                "type": "crypt",
                "offset": "16777216",
                "size": "dynamic",
                "iv_tweak": "0",
                "encryption": "aes-xts-plain64",
                "sector_size": 512
                }
            },
            "digests": {
                "0": {
                "type": "pbkdf2",
                "keyslots": [
                    "2"
                ],
                "segments": [
                    "0"
                ],
                "hash": "sha512",
                "iterations": 160039,
                "salt": "MvbiBEkmWJamhQzPZWqwLn+bTumgznQ5xc6qSw8JWX8=",
                "digest": "q20q8T3wEvpdFn3sBG27iW5lldT4t6xlzyzmN5zHMQ4ScqRzUJisIIOPvOz1lYfEAuxlxad9Si/C4zNI0rxpdQ=="
                }
            },
            "config": {
                "json_size": "12288",
                "keyslots_size": "16744448"
            }
        }

    """
    # Running this command requires additional SELinux permission for lvm_t, so temporarily switch to Permissive mode
    # Missing permission: allow lvm_t initrc_runtime_t:dir { read }
    enforcing = sudo(connection, "getenforce").strip() == "Enforcing"
    if enforcing:
        sudo(connection, "setenforce 0")

    stdout = sudo(
        connection, f"cryptsetup luksDump --dump-json-metadata {cryptDevPath}"
    )
    dump = json.loads(stdout)

    # Revert to Enforcing mode
    if enforcing:
        sudo(connection, "setenforce 1")

    # Validate type to be pbkdf2
    actual = dump["digests"]["0"]["type"]
    expected = "pbkdf2"
    assert (
        actual == expected
    ), f"Expected digest type to be {expected!r}, got {actual!r}"

    # Validate hash type to be sha512
    actual = dump["digests"]["0"]["hash"]
    expected = "sha512"
    assert (
        actual == expected
    ), f"Expected digest hash to be {expected!r}, got {actual!r}"

    # Because we first enroll encrypted volumes to PCR 0 and only then
    # re-enroll to a pcrlock policy, we expect to see only token 1.
    assert (
        len(dump["tokens"]) == 1
    ), f"Expected one token, got {len(dump['tokens'])}. Tokens: {dump['tokens']}"

    # Validate token type to be systemd-tpm2
    actual = dump["tokens"]["1"]["type"]
    expected = "systemd-tpm2"
    assert actual == expected, f"Expected token type to be {expected!r}, got {actual!r}"

    # Validate that keyslot is 2 b/c it's the 2nd keyslot used
    actual = dump["tokens"]["1"]["keyslots"][0]
    expected = "2"
    assert actual == expected, f"Expected keyslot to be {expected!r}, got {actual!r}"

    assert (
        len(dump["tokens"]["1"]["keyslots"]) == 1
    ), f"Expected one TPM keyslot, got {len(dump['tokens']['1']['keyslots'])}"

    # Validate that tpm2-pcrs is an empty vector b/c we enrolled with a pcrlock
    # policy
    assert dump["tokens"]["1"]["tpm2-pcrs"] == []

    # Validate that tpm2_pcrlock is true b/c we enrolled with a pcrlock
    # policy
    actual = dump["tokens"]["1"]["tpm2_pcrlock"]
    assert actual is True, f"Expected 'tpm2_pcrlock' to be {expected!r}, got {actual!r}"

    # Validate that there is only one keyslot
    assert (
        len(dump["keyslots"]) == 1
    ), f"Expected one keyslot, got {len(dump['keyslots'])}"
    assert (
        "2" in dump["keyslots"]
    ), f"Expected keyslot 2 to be in {dump['keyslots']!r}, got {dump['keyslots']!r}"

    # Validate that type of keyslot is luks2
    expected = "luks2"
    actual = dump["keyslots"]["0"]["type"]
    assert (
        actual == expected
    ), f"Expected keyslot 0 type to be {expected!r}, got {actual!r}"

    # Validate other info about the keyslot
    expected = "pbkdf2"
    actual = dump["keyslots"]["0"]["kdf"]["type"]
    assert (
        actual == expected
    ), f"Expected keyslot 0 KDF type to be {expected!r}, got {actual!r}"

    expected = "sha512"
    actual = dump["keyslots"]["0"]["kdf"]["hash"]
    assert (
        actual == expected
    ), f"Expected keyslot 0 KDF hash to be {expected!r}, got {actual!r}"

    expected = "aes-xts-plain64"
    actual = dump["keyslots"]["0"]["area"]["encryption"]
    assert (
        actual == expected
    ), f"Expected keyslot 0 area encryption to be {expected!r}, got {actual!r}"


def check_parent_devices(
    connection: fabric.Connection,
    hostConfiguration: dict,
    blockDevs: dict,
    cryptDevId: str,
) -> None:
    """
    Check the backing device type for the given crypt device ID.
    It can be either a disk partition or a RAID array. If a RAID
    """

    part = get_disk_partition(hostConfiguration, cryptDevId)
    if part is not None:
        cryptDevPath = get_block_dev_path_by_partlabel(blockDevs, cryptDevId)
        assert (
            cryptDevPath is not None
        ), f"Expected device with PARTLABEL {cryptDevId} to be in {blockDevs}"
    else:
        cryptDevName = get_raid_software_array_name(hostConfiguration, cryptDevId)
        assert (
            cryptDevName is not None
        ), f"Expected {cryptDevId} to be a disk partition or RAID array"
        cryptDevPath = sudo(connection, f"readlink -f /dev/md/{cryptDevName}")

    expectedType = "crypto_LUKS"
    actualType = blockDevs[cryptDevPath]["TYPE"]
    assert (
        actualType == expectedType
    ), f"Expected TYPE to be {expectedType!r}, got {actualType!r}"

    check_crypsetup_luks_dump(connection, cryptDevPath)


def check_crypt_device(
    connection: fabric.Connection,
    hostConfiguration: dict,
    abActiveVolume: str,
    blockDevs: dict,
    cryptId: str,
    cryptDevName: str,
    cryptDevId: str,
) -> None:
    cryptDevicePath = f"/dev/mapper/{cryptDevName}"

    check_parent_devices(connection, hostConfiguration, blockDevs, cryptDevId)

    swap = False
    isInUse = True

    childAbUpdateVolumePair, isVolumeA = get_child_ab_update_volume_pair(
        hostConfiguration, cryptId
    )
    if childAbUpdateVolumePair is not None:
        assert abActiveVolume in [
            "volume-a",
            "volume-b",
        ], f"Expected active volume to be either 'volume-a' or 'volume-b', got {abActiveVolume!r}"
        isInUse = (abActiveVolume == "volume-a" and isVolumeA) or (
            abActiveVolume == "volume-b" and not isVolumeA
        )

        fs = get_filesystem(hostConfiguration, childAbUpdateVolumePair["id"])
        assert (
            fs is not None
        ), f"Expected filesystem for child ab update volume pair {childAbUpdateVolumePair['id']}"
        assert (
            "mountPoint" in fs
        ), f"Expected mount point for child ab update volume pair {childAbUpdateVolumePair['id']}"
        mpPath = (
            fs["mountPoint"]
            if isinstance(fs["mountPoint"], str)
            else fs["mountPoint"]["path"]
        )
        check_exists(connection, mpPath)
        check_findmnt(connection, mpPath, cryptDevicePath, isInUse)
    elif swap := get_swap(hostConfiguration, cryptId) is not None:
        swaps = get_active_swaps(connection)
        real_path = sudo(connection, f"readlink -f {cryptDevicePath}")
        assert (
            real_path in swaps,
            f"Expected '{real_path}' to be in active swaps: {swaps}",
        )
    else:
        fs = get_filesystem(hostConfiguration, cryptId)
        assert (
            fs is not None
        ), f"Expected filesystem for encryption volume {cryptId} when it has no child ab update volume pair"

        assert (
            "mountPoint" in fs,
            f"Expected filesystem of encryption volume {cryptId} to be mounted",
        )

        mpPath = (
            fs["mountPoint"]
            if isinstance(fs["mountPoint"], str)
            else fs["mountPoint"]["path"]
        )

        check_exists(connection, mpPath)
        check_findmnt(connection, mpPath, cryptDevicePath, isInUse)

    check_exists(connection, cryptDevicePath)
    check_cryptsetup_status(connection, cryptDevName, isInUse)
    check_dmsetup_info(connection, cryptDevName, swap)


def test_encryption(
    connection: fabric.Connection,
    hostConfiguration: dict,
    abActiveVolume: str,
) -> None:
    blockDevs = get_blkid_output(connection)

    storageConfig = hostConfiguration["storage"]
    encryptionConfig = storageConfig["encryption"]
    for crypt in encryptionConfig["volumes"]:
        check_crypt_device(
            connection,
            hostConfiguration,
            abActiveVolume,
            blockDevs,
            crypt["id"],
            crypt["deviceName"],
            crypt["deviceId"],
        )
