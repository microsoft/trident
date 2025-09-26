import pytest

from base_test import get_host_status

pytestmark = [pytest.mark.ab_update_staged]


def test_ab_update_staged(connection, tridentCommand, abActiveVolume):
    # Check Host Status
    host_status = get_host_status(connection, tridentCommand)

    # Assert that servicing state is correct
    assert host_status["servicingState"] == "ab-update-staged"

    # Assert that the active volume has not changed
    assert host_status["abActiveVolume"] == abActiveVolume
