import fabric
import pytest

from base_test import get_host_status

pytestmark = [pytest.mark.rollback]


def test_rollback(
    connection: fabric.Connection,
    tridentCommand: str,
    abActiveVolume: str,
) -> None:
    # Check Host Status
    host_status = get_host_status(connection, tridentCommand)

    # Assert that servicing state is correct
    assert host_status["servicingState"] == "provisioned"

    # Assert that the active volume has not changed
    assert host_status["abActiveVolume"] == abActiveVolume
