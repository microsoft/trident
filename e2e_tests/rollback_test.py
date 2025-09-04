import json
import typing
import fabric
import pytest

from base_test import get_host_status

pytestmark = [pytest.mark.rollback]


def test_rollback(
    connection: fabric.Connection,
    hostConfiguration: dict,
    isUki: bool,
    tridentCommand: str,
    abActiveVolume: str,
) -> None:
    assert (True, f"Test to be written")
