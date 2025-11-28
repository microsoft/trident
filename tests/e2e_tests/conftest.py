from fabric import Connection, Config
import json
import os
import pytest
import yaml

# A key in the following path and the user name in the hostConfiguration are expected
file_directory_path = os.path.dirname(os.path.realpath(__file__))
key_path = os.path.join(file_directory_path, "helpers/key")
USERNAME = "testing-user"
TRIDENT_EXECUTABLE_PATH = "/usr/bin/trident"
# Expected location of Docker image:
DOCKER_IMAGE_PATH = "/var/lib/trident/trident-container.tar.gz"
EXECUTE_TRIDENT_CONTAINER = (
    "docker run --pull=never --rm --privileged "
    "-v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident "
    "-v /:/host -v /dev:/dev -v /run:/run -v /sys:/sys -v /var/log:/var/log "
    "--pid host --ipc host trident/trident:latest"
)


def pytest_addoption(parser):
    parser.addoption(
        "-H",
        "--host",
        action="store",
        type=str,
        required=True,
        help="Specify the IP address or hostname of the target machine.",
    )
    parser.addoption(
        "-C",
        "--configuration",
        action="store",
        type=str,
        required=True,
        help="Provide the path to the directory with the Host Configuration and compatible tests.",
    )
    parser.addoption(
        "-A",
        "--ab-active-volume",
        action="store",
        type=str,
        default="volume-a",
        help="Active A/B volume on the host.",
    )
    parser.addoption(
        "-K",
        "--keypath",
        action="store",
        type=str,
        default=key_path,
        help="Path to the rsa key needed for SSH connection, default path to ./keys/key.",
    )
    parser.addoption(
        "-R",
        "--runtime-env",
        action="store",
        type=str,
        choices=["host", "container"],
        default="host",
        help="Runtime environment for trident: 'host' or 'container'. Default is 'host'.",
    )
    parser.addoption(
        "-S",
        "--expected-host-status-state",
        action="store",
        type=str,
        default="provisioned",
        help="Expected host status state.",
    )


@pytest.fixture(scope="session")
def connection(request):
    host = request.config.getoption("--host")
    rsa_key = os.path.expanduser(request.config.getoption("--keypath"))
    runtime_env = request.config.getoption("--runtime-env")

    config = Config(overrides={"connect_kwargs": {"key_filename": rsa_key}})
    ssh_connection = Connection(host=host, user=USERNAME, config=config)

    # Ensure that we can connect
    ssh_connection.open()
    ssh_connection.run("hostname")

    if runtime_env == "container":
        getenforce_result = ssh_connection.run("sudo getenforce")
        # The getenforce command returns Enforcing, Permissive, or Disabled ...
        # disable if selinux is not already.
        if not "Disabled" in getenforce_result.stdout:
            # Disable SELinux
            disable_selinux_enforcement_command = "setenforce 0"
            ssh_connection.run(f"sudo {disable_selinux_enforcement_command}")
        # Load Docker Image
        load_container = f"docker load --input {DOCKER_IMAGE_PATH}"
        ssh_connection.run(f"sudo {load_container}")

    yield ssh_connection
    ssh_connection.close()


@pytest.fixture
def tridentCommand(request):
    runtime_env = request.config.getoption("--runtime-env")

    trident_command = (
        f"sudo {EXECUTE_TRIDENT_CONTAINER} "
        if runtime_env == "container"
        else f"sudo {TRIDENT_EXECUTABLE_PATH} "
    )

    return trident_command


@pytest.fixture
def hostConfiguration(request):
    file_path = request.config.getoption("--configuration")
    tridentconfig_path = os.path.join(file_path, "trident-config.yaml")
    with open(tridentconfig_path, "r") as stream:
        try:
            trident_Configuration = yaml.safe_load(stream)
        except yaml.YAMLError as exc:
            print(exc)
            return {}

    return trident_Configuration


@pytest.fixture
def isUki(request):
    file_path = request.config.getoption("--configuration")
    testselection_path = os.path.join(file_path, "test-selection.yaml")
    with open(testselection_path, "r") as stream:
        try:
            test_Selection = yaml.safe_load(stream)
        except yaml.YAMLError as exc:
            print(exc)
            return {}

    return "uki" in test_Selection.get("compatible", [])


@pytest.fixture
def abActiveVolume(request):
    return request.config.getoption("--ab-active-volume")


@pytest.fixture
def expectedHostStatusState(request):
    return request.config.getoption("--expected-host-status-state")


def define_tests(file_path):
    with open(file_path, "r") as stream:
        try:
            test_markers = yaml.safe_load(stream)
        except yaml.YAMLError as exc:
            print(exc)
            return

    # Define tests:
    special_markers = [
        "compatible",
        "weekly",
        "daily",
        "post_merge",
        "pullrequest",
        "validation",
    ]  # The order matters here; each element depends on the previous ones.
    actions = ["add", "remove"]
    types = ["modules", "markers"]
    # Structure:
    tests_selected = {
        special_marker: {action: {tp: set() for tp in types} for action in actions}
        for special_marker in special_markers
    }
    # Add information
    test_markers["compatible"] = {"add": test_markers.get("compatible", list())}
    for test_marker, test_marker_value in test_markers.items():
        for action, action_value in test_marker_value.items():
            for element in action_value:
                if "::" in element:
                    tests_selected[test_marker][action]["modules"].add(element)
                else:
                    tests_selected[test_marker][action]["markers"].add(element)
    return tests_selected


def pytest_collection_modifyitems(config, items):
    configuration_path = config.getoption("--configuration")
    tests_path = os.path.join(configuration_path, "test-selection.yaml")
    test_markers = define_tests(tests_path)

    # Add special markers to functions (tests)
    modules_per_marker = {marker: set() for marker in test_markers}
    current_definition = set()
    for marker_name, marker_tests in test_markers.items():
        special_marker = getattr(pytest.mark, marker_name)
        for item in items:
            item_markers = set(item.keywords)
            if item.nodeid in marker_tests["add"]["modules"]:
                item.add_marker(special_marker)
                modules_per_marker[marker_name].add(item.nodeid)
                continue
            if item.nodeid in marker_tests["remove"]["modules"]:
                continue
            if item_markers & marker_tests["remove"]["markers"]:
                continue
            if item_markers & marker_tests["add"]["markers"]:
                item.add_marker(special_marker)
                modules_per_marker[marker_name].add(item.nodeid)
                continue
            if item.nodeid in current_definition:
                item.add_marker(special_marker)
                modules_per_marker[marker_name].add(item.nodeid)
                continue
        current_definition = modules_per_marker[marker_name]

    # Save the tests selected by each special marker
    marker_file_test = dict()
    for marker, modules in modules_per_marker.items():
        marker_file_test[marker] = dict()
        for module in modules:
            module_file, module_function = module.split("::", 1)
            if not module_file in marker_file_test[marker]:
                marker_file_test[marker][module_file] = list()
            marker_file_test[marker][module_file].append(module_function)
    mft_file = os.path.join(configuration_path, "ts.json")
    with open(mft_file, "w") as ts:
        json.dump(marker_file_test, ts, indent=4)
