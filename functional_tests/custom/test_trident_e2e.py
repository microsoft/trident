import yaml
import os
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
        result.stderr.index("Failed to run due to missing root privileges") != -1
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
    for id in host_status["blockDevicePaths"]:
        host_status["blockDevicePaths"][id] = placeholder
    host_status["disksByUuid"] = {placeholder: placeholder}
    with open(
        TRIDENT_REPO_DIR_PATH / "functional_tests/host-status-template.yaml", "r"
    ) as file:
        host_status_expected = yaml.load(file, Loader=HostStatusSafeLoader)
    assert host_status == host_status_expected

    pass


@pytest.mark.functional
@pytest.mark.core
def test_trident_offline_initialize(vm):
    """Basic trident offline initialize validation."""
    trident = TridentTool(vm)
    host_status = trident.get()

    # Load it as a yaml
    host_status = yaml.load(host_status, Loader=HostStatusSafeLoader)

    working_dir = "/tmp/datastore"

    result = vm.execute("rm -rf " + working_dir)
    assert_that(result.exit_code).is_equal_to(0)
    vm.mkdir(working_dir)

    datastore_path = f"{working_dir}/datastore.sqlite"

    # Update the datastore location
    host_status["spec"]["trident"] = {"datastorePath": datastore_path}

    # Create mirror directory
    if not os.path.exists(working_dir):
        os.mkdir(working_dir)

    # Store it in a temporary file
    host_status_path = f"{working_dir}/host-status.yaml"
    with open(host_status_path, "w") as file:
        yaml.dump(host_status, file)
    vm.copy(host_status_path, host_status_path)

    trident.offline_initialize(host_status_path)

    vm.execute(f"sudo chown testuser {datastore_path}")

    # Create Trident config pointing to the new datastore
    trident_config_path = f"{working_dir}/trident-config.yaml"
    with open(trident_config_path, "w") as file:
        yaml.dump(
            {
                "datastore": datastore_path,
            },
            file,
        )
    vm.copy(trident_config_path, trident_config_path)

    # Use Trident get with the new config to load the status from the datastore
    loaded_host_status = yaml.load(
        trident.get(trident_config_path), Loader=HostStatusSafeLoader
    )

    host_status["spec"].pop("trident")
    loaded_host_status["spec"].pop("trident")

    # Check if the loaded status is the same as the original status
    assert host_status == loaded_host_status

    pass


@pytest.mark.functional
@pytest.mark.core
def test_trident_start_network(vm):
    """Basic trident start-network validation."""
    trident = TridentTool(vm)
    trident.start_network()

    pass
