import fabric
import pytest
import json

from base_test import get_host_status

pytestmark = [pytest.mark.extensions]


def test_extensions(
    connection: fabric.Connection,
    tridentCommand: str,
) -> None:
    hostStatus = get_host_status(connection, tridentCommand)
    hostConfig = hostStatus["spec"]
    osConfig = hostConfig["os"]

    for extType in ["sysext", "confext"]:
        configExtType = f"{extType}s"
        if configExtType in osConfig:
            extConfig = osConfig[configExtType]
            result = connection.run(
                f"sudo systemd-{extType} list --json=pretty --no-pager", warn=True
            )
            assert result.ok, f"failed to run 'systemd-{extType} list': {result.stderr}"
            ext_list = json.loads(result.stdout)

            for ext in extConfig:
                # Verify that the path exists on the target OS
                path = ext["path"]
                result = connection.run(f"test -e {path}", warn=True)
                assert result.ok, f"{extType} path does not exist: {path}"

                # Extract filename from path and check if it's in systemd-*ext list
                found = any(e.get("path") == path for e in ext_list)
                assert found, f"{extType} at {path} not found in systemd-{extType} list"
