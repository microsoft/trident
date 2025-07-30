from fabric import Connection, Config
from io import StringIO
from invoke.exceptions import CommandTimedOut
from invoke.watchers import StreamWatcher
from typing import Literal, Tuple

LOCAL_TRIDENT_CONFIG_PATH = "/etc/trident/config.yaml"
TRIDENT_EXECUTABLE_PATH = "/usr/bin/trident"
# Expected location of Docker image:
DOCKER_IMAGE_PATH = "/var/lib/trident/trident-container.tar.gz"
EXECUTE_TRIDENT_CONTAINER = (
    "docker run --pull=never --rm --privileged "
    "-v /etc/trident:/etc/trident -v /var/lib/trident:/var/lib/trident "
    "-v /:/host -v /dev:/dev -v /run:/run -v /sys:/sys -v /var/log:/var/log "
    "--pid host --ipc host trident/trident:latest"
)


class OutputWatcher(StreamWatcher):
    def __init__(self):
        super().__init__()
        self.output_len = 0

    def submit(self, stream):
        new_output = stream[self.output_len :]
        print(new_output, end="")
        self.output_len = len(stream)
        return []


def trident_run(
    connection: Connection, command: str, runtime_env: Literal["host", "container"]
) -> Tuple[int, str, str]:
    """
    Runs Trident's commands on the remote host locally or in a container.
    """
    # Initialize a watcher to return output live.
    watcher = OutputWatcher()
    # Initialize stdout and stderr streams.
    out_stream = StringIO()
    err_stream = StringIO()

    # Define how to execute Trident.
    trident_invocation = _trident_command(runtime_env, connection)

    try:
        print(f"Executing Trident run command: {command}")
        # Set warn=True to continue execution even if the command has a non-zero exit code.
        result = connection.run(
            f"sudo {trident_invocation} {command}",
            warn=True,
            out_stream=out_stream,
            err_stream=err_stream,
            timeout=240,
            watchers=[watcher],
        )
        return (result.return_code, result.stdout, result.stderr)

    # Handle the case where the command times out.
    except CommandTimedOut as timeout_exception:
        print("Timeout occurred while executing Trident run command.")
        output = out_stream.getvalue() + err_stream.getvalue()
        # Raise error with Trident's output as additional information.
        raise CommandTimedOut(output) from timeout_exception

    except Exception as e:
        print(f"Unexpected error occurred while executing Trident run command: {e}")
        raise


def _trident_command(
    runtime_env: Literal["host", "container"], connection: Connection
) -> str:
    if runtime_env == "container":
        # Load the Docker image to guarantee it will be available.
        _reload_container_image(connection)
        return EXECUTE_TRIDENT_CONTAINER
    else:
        return TRIDENT_EXECUTABLE_PATH


def _reload_container_image(connection: Connection):
    if not check_file_exists(connection, DOCKER_IMAGE_PATH):
        raise Exception(f"Can not locate Docker image at {DOCKER_IMAGE_PATH}.")

    # Disable SELinux:
    disable_selinux_enforcement_command = "sudo setenforce 0"
    _connection_run_command(
        connection, disable_selinux_enforcement_command
    )  # TODO: Re-enable SELinux (#9508).

    command = f"sudo docker load --input {DOCKER_IMAGE_PATH}"
    result = _connection_run_command(connection, command)
    if not result.ok:
        raise Exception(
            f"Unable to load Docker image for Trident.",
            f"Command: {command}",
            f"Output: {result.stdout}",
            f"Error: {result.stderr}.",
        )
    return


def run_ssh_command(connection: Connection, command: str, use_sudo=False) -> str:
    """
    Runs a command on the host and returns the combined stdout and stderr.
    """
    # Prepend 'sudo' to the command if necessary
    if use_sudo:
        command = f"sudo {command}"
    result = _connection_run_command(connection, command)

    return result.stdout + result.stderr


def check_file_exists(connection: Connection, file_path: str) -> bool:
    """
    Checks if a file exists at the specified path on the host.
    """
    command = f"test -f {file_path}"
    result = _connection_run_command(connection, command)

    return result.ok


def _connection_run_command(connection: Connection, command: str):
    try:
        # Executes a command on the host using Fabric
        result = connection.run(command, warn=True, hide="both")
        return result
    except Exception as e:
        print(f"An unexpected error occurred:\n{e}")
        raise


def create_ssh_connection(
    ip_address: str, user_name: str, keys_file_path: str
) -> Connection:
    """
    Creates and returns an SSH connection using Fabric.
    """
    config = Config(overrides={"connect_kwargs": {"key_filename": keys_file_path}})
    connection = Connection(host=ip_address, user=user_name, config=config)

    return connection
