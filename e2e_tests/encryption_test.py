import json
import typing
import fabric
import pytest

from base_test import get_host_status

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


def check_crypsetup_luks_dump(
    connection: fabric.Connection, tridentCommand: str, cryptDevPath: str
) -> None:
    """
    Check the output of `cryptsetup luksDump --dump-json-metadata` for the
    given device path. The output will differ depending on whether the
    encryption is based on a pcrlock policy or not.

    Example output for a testing flow using a UKI ROS image, where a pcrlock
    policy is used:

        {
            "keyslots":{
                "1":{
                "type":"luks2",
                "key_size":64,
                "af":{
                    "type":"luks1",
                    "stripes":4000,
                    "hash":"sha512"
                },
                "area":{
                    "type":"raw",
                    "offset":"290816",
                    "size":"258048",
                    "encryption":"aes-xts-plain64",
                    "key_size":64
                },
                "kdf":{
                    "type":"pbkdf2",
                    "hash":"sha512",
                    "iterations":1000,
                    "salt":"FHJf95bq+nk/WkCCCOIyPDwLbzpwkkiTgs2vjFZgLU0="
                }
                }
            },
            "tokens":{
                "0":{
                "type":"systemd-tpm2",
                "keyslots":[
                    "1"
                ],
                "tpm2-blob":"AJ4AIOxkvkN7ubF8IL0kItGHh411aCZdcha75buXgoErsv7XABC6LxxewHtkhfHuoZKCtWza4dBEAcfsAGJPCQfEsbBQKM4jTa5DfK2hhKw8IAw0diThe5e1zuXwtq1CLrglQV9G/rylRh3R4O8E0obRBMCk8925+FEtguQNIghRGDKQG1T+mU8UxKz/dWC1kekW861ynqZ/Qqwg+6KVowBOAAgACwAAABIAIBT0BjJdNmRaCrVDJcOjeJKAt9hhmGBJXDGstAMoyuyOABAAIFFuPmr9Q4dSL20RBcldk4EWqztfr1rFSwR7W6vC6uCC",
                "tpm2-pcrs":[],
                "tpm2-policy-hash":"14f406325d36645a0ab54325c3a3789280b7d8619860495c31acb40328caec8e",
                "tpm2-pin":false,
                "tpm2_pcrlock":true,
                "tpm2_srk":"gQAAAQAiAAv5sLge1GM24g8z8nGVwj63AzM3lF2hByQxbh7A0TfsJwAAAAEAWgAjAAsAAwRyAAAABgCAAEMAEAADABAAIFsbfkcs48O3R29GrqlI9KOrqZPkoXQQb6WcYwwN4NibACDM5iwqa8lnLk89PJ10t0O6cpBaKn3nEayvLDm/8KVV8w=="
                }
            },
            "segments":{
                "0":{
                "type":"crypt",
                "offset":"16777216",
                "size":"dynamic",
                "iv_tweak":"0",
                "encryption":"aes-xts-plain64",
                "sector_size":512
                }
            },
            "digests":{
                "0":{
                "type":"pbkdf2",
                "keyslots":[
                    "1"
                ],
                "segments":[
                    "0"
                ],
                "hash":"sha512",
                "iterations":161022,
                "salt":"oIpU6dVsGn3ulUz39RNpRpxgZ2TejXH55h+RP9VtO40=",
                "digest":"oGaQSK3jH+mGsEVDHfbks1Xkqk6OpZkK3fo428wSLInHnJ3sLPz58CESLue6g9nE795eCvbuyWPih6AGs3jVNA=="
                }
            },
            "config":{
                "json_size":"12288",
                "keyslots_size":"16744448"
            }
        }

    Example output for a grub ROS image, where pcrlock policy is NOT used, and
    instead, the volume is enrolled to the value of PCR 7:

        {
            "keyslots":{
                "1":{
                "type":"luks2",
                "key_size":64,
                "af":{
                    "type":"luks1",
                    "stripes":4000,
                    "hash":"sha512"
                },
                "area":{
                    "type":"raw",
                    "offset":"290816",
                    "size":"258048",
                    "encryption":"aes-xts-plain64",
                    "key_size":64
                },
                "kdf":{
                    "type":"pbkdf2",
                    "hash":"sha512",
                    "iterations":1000,
                    "salt":"V78EvaQMSroXSvwmIlaQ7QgJEAYdmykbr/U580bibK4="
                }
                }
            },
            "tokens":{
                "0":{
                "type":"systemd-tpm2",
                "keyslots":[
                    "1"
                ],
                "tpm2-blob":"AJ4AIA6FIxPCpzLJIrPYM+xkjHd01LZAQQjcoiK3fWJNy0zHABBErLSo75LGafmcHEIOhx7PtNoO3x4hW86gT0Jkf1drvjnULqHQBV13iJnDz1w+lbK+GnfumBntmj12LLeUIr/6SAVCU+KNu/owZCyOl1+p1eLFNt9LwpXQLCUa4rfPYUsLHYG/TcNc9kzQcpw49TEFRJwADQUiZlD1XgBOAAgACwAAABIAINyyj1eDDYXFHzMdKs/hxGS6hMdir2JqwbiulxUWj5RIABAAIJaw5Gip7m2PDETPSC/HOEZGosLuCpDGQ6sms6RwVdGh",
                "tpm2-pcrs":[],
                "tpm2-policy-hash":"dcb28f57830d85c51f331d2acfe1c464ba84c762af626ac1b8ae9715168f9448",
                "tpm2-pin":false,
                "tpm2_pcrlock":true,
                "tpm2_srk":"gQAAAQAiAAuRnxBWvxRchDQNQyi/ryIVqTKLSmwcmfXCqzpmf3Ls7QAAAAEAWgAjAAsAAwRyAAAABgCAAEMAEAADABAAILIu6HvU3U/n+AclA9T/nOQ8gVGaNIgAGWScI5CThurRACCEWbjxEE50DKczUwuOXAd0/iCEid83UE10zB6ncOzYJA=="
                }
            },
            "segments":{
                "0":{
                "type":"crypt",
                "offset":"16777216",
                "size":"dynamic",
                "iv_tweak":"0",
                "encryption":"aes-xts-plain64",
                "sector_size":512
                }
            },
            "digests":{
                "0":{
                "type":"pbkdf2",
                "keyslots":[
                    "1"
                ],
                "segments":[
                    "0"
                ],
                "hash":"sha512",
                "iterations":158875,
                "salt":"OsbDAAnbzyWuugQsSF1E+EphOH/Oxw+IhsPd7rw7dFA=",
                "digest":"gVqfej2XffVQR3FEMgSA19WZgKtcfETrfAThRlao86TdjaU/vUyGRoMrshL8zEULAwSORd9qiuZ2gPPN4fu1XA=="
                }
            },
            "config":{
                "json_size":"12288",
                "keyslots_size":"16744448"
            }
        }

    """
    # Running this command requires additional SELinux permission for lvm_t:
    # allow lvm_t initrc_runtime_t:dir { read }.
    # This is a quirk of the testing infra, and this perm shouldn't be part of
    # the Trident policy. So, temporarily switch to Permissive mode.
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

    # Validate type of digest to be pbkdf2
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

    # Check Host Status to see if image is UKI or not
    host_status = get_host_status(connection, tridentCommand)
    # TODO: Remove this override once UKI & encryption tests are fixed. ADO:
    # https://dev.azure.com/mariner-org/polar/_workitems/edit/13344/.
    override_uki = (
        host_status["spec"]
        .get("internalParams", {})
        .get("overridePcrlockEncryption", False)
    )
    is_uki = (
        host_status["spec"].get("internalParams", {}).get("uki", False)
        and not override_uki
    )

    # For both UKI and grub ROS images, we expect to see a single token 1
    assert (
        "0" in dump["tokens"]
    ), f"Expected token 0 to be in {dump['tokens']!r}, got {dump['tokens']!r}"
    assert (
        "1" in dump["tokens"]["0"]["keyslots"]
    ), f"Expected key slot 1 to be in {dump['tokens']['0']['keyslots']!r}, got {dump['tokens']['0']['keyslots']!r}"
    assert (
        len(dump["tokens"]) == 1
    ), f"Expected one token, got {len(dump['tokens'])}. Tokens: {dump['tokens']}"
    assert (
        len(dump["tokens"]["0"]["keyslots"]) == 1
    ), f"Expected one key slot for the token, got {len(dump['tokens']['0']['keyslots'])}. Key slots: {dump['tokens']['0']['keyslots']}"

    # Validate token type to be systemd-tpm2
    actual = dump["tokens"]["0"]["type"]
    expected = "systemd-tpm2"
    assert actual == expected, f"Expected token type to be {expected!r}, got {actual!r}"

    # Validate that for UKI images, tpm2_pcrlock is true and tpm2-pcrs is an
    # empty vector, while for non-UKI images, tpm2_pcrlock is false and
    # tpm2-pcrs is a vector with PCR 7 or 0.
    # # TODO: Once tests are fixed, we would only expect to see PCR 7 here.
    # Related ADO task:
    # https://dev.azure.com/mariner-org/polar/_workitems/edit/13344/.
    if is_uki:
        assert (
            dump["tokens"]["0"]["tpm2_pcrlock"] is True
        ), f"Expected tpm2_pcrlock to be True for UKI image, got {dump['tokens']['0']['tpm2_pcrlock']!r}"
        assert (
            dump["tokens"]["0"]["tpm2-pcrs"] == []
        ), f"Expected tpm2-pcrs to be an empty vector for UKI image, got {dump['tokens']['0']['tpm2-pcrs']!r}"
    else:
        assert (
            dump["tokens"]["0"]["tpm2_pcrlock"] is False
        ), f"Expected tpm2_pcrlock to be False for non-UKI image, got {dump['tokens']['0']['tpm2_pcrlock']!r}"
        # Expect PCR 7 or 0
        assert dump["tokens"]["0"]["tpm2-pcrs"] == [7] or dump["tokens"]["0"][
            "tpm2-pcrs"
        ] == [
            0
        ], f"Expected tpm2-pcrs to be [7] or [0] for non-UKI image, got {dump['tokens']['0']['tpm2-pcrs']!r}"

    # Validate that each image has a single keyslot, 1
    assert (
        len(dump["keyslots"]) == 1
    ), f"Expected one key slot, got {len(dump['keyslots'])}. Key slots: {dump['keyslots']}"
    assert (
        "1" in dump["keyslots"]
    ), f"Expected key slot 1 to be in {dump['keyslots']!r}, got {dump['keyslots']!r}"

    # Validate key slot type and other properties
    expected = "luks2"
    actual = dump["keyslots"]["1"]["type"]
    assert (
        actual == expected
    ), f"Expected keyslot 1 type to be {expected!r}, got {actual!r}"

    # Validate key slot KDF type
    expected = "pbkdf2"
    actual = dump["keyslots"]["1"]["kdf"]["type"]
    assert (
        actual == expected
    ), f"Expected keyslot 1 KDF type to be {expected!r}, got {actual!r}"

    # Validate key slot KDF hash
    expected = "sha512"
    actual = dump["keyslots"]["1"]["kdf"]["hash"]
    assert (
        actual == expected
    ), f"Expected keyslot 1 KDF hash to be {expected!r}, got {actual!r}"

    # Validate key slot area type
    expected = "aes-xts-plain64"
    actual = dump["keyslots"]["1"]["area"]["encryption"]
    assert (
        actual == expected
    ), f"Expected keyslot 1 area encryption to be {expected!r}, got {actual!r}"


def check_parent_devices(
    connection: fabric.Connection,
    hostConfiguration: dict,
    tridentCommand: str,
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

    check_crypsetup_luks_dump(connection, tridentCommand, cryptDevPath)


def check_crypt_device(
    connection: fabric.Connection,
    hostConfiguration: dict,
    tridentCommand: str,
    abActiveVolume: str,
    blockDevs: dict,
    cryptId: str,
    cryptDevName: str,
    cryptDevId: str,
) -> None:
    cryptDevicePath = f"/dev/mapper/{cryptDevName}"

    check_parent_devices(
        connection, hostConfiguration, tridentCommand, blockDevs, cryptDevId
    )

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
    tridentCommand: str,
    abActiveVolume: str,
) -> None:
    blockDevs = get_blkid_output(connection)

    storageConfig = hostConfiguration["storage"]
    encryptionConfig = storageConfig["encryption"]
    for crypt in encryptionConfig["volumes"]:
        check_crypt_device(
            connection,
            hostConfiguration,
            tridentCommand,
            abActiveVolume,
            blockDevs,
            crypt["id"],
            crypt["deviceName"],
            crypt["deviceId"],
        )
