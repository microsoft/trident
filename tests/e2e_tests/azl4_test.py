import shlex

import pytest

pytestmark = [pytest.mark.azl4]


def test_azl4(connection) -> None:
    os_release = dict(
        entry.split("=", 1)
        for entry in shlex.split(
            connection.run("cat /etc/os-release").stdout, comments=True
        )
    )

    assert os_release["ID"] == "azurelinux"
    assert os_release["VERSION_ID"] == "4.0"
