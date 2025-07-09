import pytest
import yaml
from typing import Dict, List, Tuple

pytestmark = [pytest.mark.ab_update_staged]


def test_ab_update_staged(
    connection, hostConfiguration, tridentCommand, abActiveVolume
):
    # Check Host Status.
    trident_get_command = tridentCommand + "get"
    res_host_status = connection.run(trident_get_command)
    output_host_status = res_host_status.stdout.strip()

    yaml.add_multi_constructor(
        "!", lambda loader, _, node: loader.construct_mapping(node)
    )
    host_status = yaml.load(output_host_status, Loader=yaml.FullLoader)

    # Assert that servicingState is correct.
    assert host_status["servicingState"] == "ab-update-staged"

    # Assert that the active volume has not changed.
    assert host_status["abActiveVolume"] == abActiveVolume
