import fabric
import pytest

from base_test import get_host_status

pytestmark = [pytest.mark.uefifallback]


def test_uefi_fallback(
    connection: fabric.Connection,
    tridentCommand: str,
    abActiveVolume: str,
) -> None:
    # Check Host Status
    host_status = get_host_status(connection, tridentCommand)
    # Assert that the active volume has not changed
    assert host_status["abActiveVolume"] == abActiveVolume
