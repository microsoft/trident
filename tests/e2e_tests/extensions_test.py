import fabric
import pytest
import json
import os

from base_test import get_host_status

pytestmark = [pytest.mark.extensions]


def test_extensions(
    connection: fabric.Connection,
    tridentCommand: str,
) -> None:
    hostStatus = get_host_status(connection, tridentCommand)
    hostConfig = hostStatus["spec"]
    osConfig = hostConfig["os"]

    if "sysexts" in osConfig:
        sysextsConfig = osConfig["sysexts"]

        result = connection.run(
            "systemd-sysext list --json=pretty --no-pager", warn=True
        )
        assert result.ok, "Failed to run 'systemd-sysext list'"
        sysext_list = json.loads(result.stdout)

        for sysext in sysextsConfig:
            # Verify that the path exists on the target OS
            path = sysext["path"]
            result = connection.run(f"test -e {path}", warn=True)
            assert result.ok, f"sysext path does not exist: {path}"

            # Extract filename from path and check if its in systemd-sysext list
            found = any(ext.get("path") == path for ext in sysext_list)
            assert found, f"systext at {path} not found in systemd-sysext list"
    if "confexts" in osConfig:
        confextsConfig = osConfig["confexts"]
        for confext in confextsConfig:
            # Verify that the path exists on the target OS
            path = confext["path"]
            result = connection.run(f"test -e {path}", warn=True)
            assert result.ok, f"confext path does not exist: {path}"
