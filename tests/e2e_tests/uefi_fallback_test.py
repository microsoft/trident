import json
import typing
import fabric
import pytest

from base_test import get_host_status

pytestmark = [pytest.mark.uefifallback]


def test_uefi_fallback(
    connection: fabric.Connection,
    hostConfiguration: dict,
    isUefiFallback: bool,
    tridentCommand: str,
    abActiveVolume: str,
) -> None:
    if not isUefiFallback:
        pytest.skip("Skipping test since not compatible with uefi-fallback")

    host_status = get_host_status(connection, tridentCommand)
    assert host_status["boot"]["ab_active_volume"] == abActiveVolume
