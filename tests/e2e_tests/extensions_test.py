import fabric
import pytest
import json

from base_test import get_host_status
from pathlib import Path

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
                f"sudo systemd-{extType} status --json=pretty --no-pager", warn=True
            )
            assert (
                result.ok
            ), f"failed to run 'systemd-{extType} status': {result.stderr}"
            status_output = json.loads(result.stdout)

            # Extract all active extension names from all hierarchies (/opt and
            # /usr for sysexts, or /etc for confexts)
            active_exts = []
            for hierarchy in status_output:
                extensions = hierarchy.get("extensions")
                if isinstance(extensions, list):
                    active_exts.extend(extensions)

            for ext in extConfig:
                # Verify that the path exists on the target OS
                path = Path(ext["path"])
                result = connection.run(f"test -e {path}", warn=True)
                assert result.ok, f"{extType} path does not exist: {path}"

                # Extract extension name from path
                ext_name = path.stem
                assert (
                    ext_name in active_exts
                ), f"{extType} '{ext_name}' not found in 'systemd-{extType} status'"
