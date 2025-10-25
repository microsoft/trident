import fabric
import pytest
import yaml

from base_test import get_host_status

pytestmark = [pytest.mark.uefifallback]


def test_uefi_fallback(
    connection: fabric.Connection,
    tridentCommand: str,
    abActiveVolume: str,
) -> None:
    # Check Host Status
    host_status = get_host_status(connection, tridentCommand)
    # Assert that the servicing state is provisioned
    assert host_status["servicingState"] == "provisioned"

    activeVolume = abActiveVolume
    scenario = "clean-install"
    if activeVolume.startswith("abupdate-"):
        # Strip the abupdate- prefix to get the actual volume name
        activeVolume = activeVolume[len("abupdate-") :]
        scenario = "abupdate"

    if scenario == "abupdate":
        # Assert that the last error reflects health.checks failure
        serializedLastError = yaml.dump(
            host_status["lastError"], default_flow_style=False
        )
        assert "A/B update failed as host booted from" in serializedLastError
    else:
        assert host_status["lastError"] is None

    # Assert that the active volume has not changed
    assert host_status["abActiveVolume"] == activeVolume
