import yaml
import pytest

from assertpy import assert_that  # type: ignore

from functional_tests.tools.trident import TridentTool
from functional_tests.conftest import TRIDENT_REPO_DIR_PATH


class HostStatusSafeLoader(yaml.SafeLoader):
    def accept_image(self, node):
        return self.construct_mapping(node)


HostStatusSafeLoader.add_constructor("!image", HostStatusSafeLoader.accept_image)


@pytest.mark.functional
@pytest.mark.core
def test_trident_run(vm):
    """Basic trident run validation."""
    trident = TridentTool(vm)
    result = trident.run()
    assert_that(result.exit_code).is_equal_to(0)

    result = trident.run(False)
    assert_that(result.exit_code).is_equal_to(2)
    assert_that(
        result.stderr.index(
            "Selected operation cannot be performed due to missing permissions, root privileges required"
        )
        != -1
    )

    pass


@pytest.mark.functional
@pytest.mark.core
def test_trident_get(vm):
    """Basic trident get validation."""
    trident = TridentTool(vm)

    host_status = trident.get()
    host_status = yaml.load(host_status, Loader=HostStatusSafeLoader)
    # TODO remove the placeholder logic by patching the template with the actual
    # values, which we can fetch using lsblk, sfdisk and information about the
    # images we put into the HostConfiguraion.
    del host_status["spec"]
    placeholder = "placeholder"
    for id, block_device in host_status["storage"]["blockDevices"].items():
        block_device["path"] = placeholder
        if (
            isinstance(block_device["contents"], dict)
            and "sha256" in block_device["contents"]
        ):
            block_device["contents"]["sha256"] = placeholder
            block_device["contents"]["length"] = placeholder
            block_device["contents"]["url"] = placeholder
    host_status["storage"]["diskUuidIdMap"] = {placeholder: placeholder}
    with open(
        TRIDENT_REPO_DIR_PATH / "functional_tests/host-status-template.yaml", "r"
    ) as file:
        host_status_expected = yaml.load(file, Loader=HostStatusSafeLoader)
    assert host_status == host_status_expected

    pass


@pytest.mark.functional
@pytest.mark.core
def test_trident_start_network(vm):
    """Basic trident start-network validation."""
    trident = TridentTool(vm)
    trident.start_network()

    pass
