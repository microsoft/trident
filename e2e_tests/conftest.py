from fabric import Connection, Config
import json
import os
import pytest
import yaml


# A key in the following path and the user name in the hostConfiguration are expected
FILE_DIRECTORY_PATH = os.path.dirname(os.path.realpath(__file__))
KEY_PATH = os.path.join(FILE_DIRECTORY_PATH, "helpers/key")
USERNAME = "testing-user"


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
        help="Provide the path to the directory with the trident configuration and compatible tests.",
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
        default=KEY_PATH,
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


@pytest.fixture
def connection(request):
    HOST = request.config.getoption("--host")
    RSA_KEY = os.path.expanduser(request.config.getoption("--keypath"))

    config = Config(overrides={"connect_kwargs": {"key_filename": RSA_KEY}})

    ssh_connection = Connection(host=HOST, user=USERNAME, config=config)

    # Ensure that we can connect
    ssh_connection.open()
    ssh_connection.run("hostname")

    yield ssh_connection
    ssh_connection.close()


@pytest.fixture
def tridentCommand(request):
    RUNTIME_ENV = request.config.getoption("--runtime-env")

    trident_command = "sudo "
    if RUNTIME_ENV == "container":
        # Start the Docker container
        run_container = (
            "docker run --pull=never --rm --privileged "
            "-v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident "
            "-v /:/host -v /dev:/dev -v /run:/run -v /sys:/sys -v /var/log:/var/log "
            "--pid host trident/trident:latest"
        )
        trident_command += run_container + " "
    else:
        TRIDENT_EXECUTABLE_PATH = "/usr/bin/trident"
        trident_command += TRIDENT_EXECUTABLE_PATH + " "

    return trident_command


@pytest.fixture
def tridentConfiguration(request):
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
def abActiveVolume(request):
    return request.config.getoption("--ab-active-volume")


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
