import pytest
import yaml
from typing import Dict, List, Tuple

pytestmark = [pytest.mark.ab_update_staged]


class HostStatusSafeLoader(yaml.SafeLoader):
    def accept_image(self, node):
        return self.construct_mapping(node)


def test_ab_update_staged(connection, tridentConfiguration, abActiveVolume):
    # Check host status.
    res_host_status = connection.run("sudo /usr/bin/trident get")
    output_host_status = res_host_status.stdout.strip()

    HostStatusSafeLoader.add_constructor("!image", HostStatusSafeLoader.accept_image)
    host_status = yaml.load(output_host_status, Loader=HostStatusSafeLoader)

    # Assert that servicingType and serviceState are correct.
    assert host_status["servicingType"] == "ab-update"
    assert host_status["servicingState"] == "staged"

    # Assert that the active volume has not changed.
    assert host_status["storage"]["abActiveVolume"] == abActiveVolume
