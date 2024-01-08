import pytest
import os
from fabric import Connection, Config
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
        "-T",
        "--tridentconfig",
        action="store",
        type=str,
        required=True,
        help="Provide the path to the trident configuration.",
    )
    parser.addoption(
        "-K",
        "--keypath",
        action="store",
        type=str,
        default=KEY_PATH,
        help="Path to the rsa key needed for conection, default path to ./keys/key",
    )


@pytest.fixture
def connection(request):
    HOST = request.config.getoption("--host")
    RSA_KEY = os.path.expanduser(request.config.getoption("--keypath"))

    config = Config(overrides={"connect_kwargs": {"key_filename": RSA_KEY}})

    ssh_connection = Connection(host=HOST, user=USERNAME, config=config)
    yield ssh_connection
    ssh_connection.close()


@pytest.fixture
def tridentConfiguration(request):
    file_path = request.config.getoption("--tridentconfig")
    with open(file_path, "r") as stream:
        try:
            trident_Configuration = yaml.safe_load(stream)
        except yaml.YAMLError as exc:
            print(exc)
            return {}

    return trident_Configuration
