#!/usr/bin/env python3
import argparse
from fabric import Connection, Config
from invoke.watchers import StreamWatcher
from io import StringIO
from ssh_utilities import (
    run_ssh_command,
    OutputWatcher,
    TRIDENT_EXECUTABLE_PATH,
    LOCAL_TRIDENT_CONFIG_PATH,
)


def trident_rebuild_raid(connection, trident_config):
    """
    Runs "trident rebuild-raid" to trigger rebuilding RAID and checks if RAID was rebuilt successfully.

    Args:
        connection : The SSH connection to the host.
        trident_config : The full path to the Trident config on the host.

    """
    # Initialize a watcher to return output live
    watcher = OutputWatcher()
    # Initialize stdout and stderr streams
    out_stream = StringIO()
    err_stream = StringIO()
    try:
        # Set warn=True to continue execution even if the command has a non-zero exit code.
        # Provide -c arg, the full path to the RW Trident config.
        result = connection.run(
            f"sudo {TRIDENT_EXECUTABLE_PATH} rebuild-raid -v trace -c {trident_config}",
            warn=True,
            out_stream=out_stream,
            err_stream=err_stream,
            timeout=180,
            watchers=[watcher],
        )

    except Exception as e:
        print(f"An unexpected error occurred:\n")
        raise

    finally:
        connection.close()

    output = result.stdout + result.stderr
    print("Trident rebuild-raid output {}".format(output))

    # Check the exit code: if 0, Trident rebuild-raid succeeded.
    if result.return_code == 0:
        print(
            "Received expected output with exit code 0. Trident rebuild-raid succeeded."
        )
    else:
        raise Exception(
            f"Command unexpectedly returned with exit code {result.return_code} and output {output}"
        )

    return


def copy_host_config(connection, trident_config):
    """
    Copies the Trident config to the host.

    Args:
        connection : The SSH connection to the host.
        trident_config : The full path to the Trident config on the host.

    """

    # If file at path trident_config does not exist, copy it over from LOCAL_TRIDENT_CONFIG_PATH
    result = connection.run(f"test -f {trident_config}", warn=True, hide="both")
    if not result.ok:
        print(
            f"File {trident_config} does not exist. Copying from {LOCAL_TRIDENT_CONFIG_PATH}"
        )
        run_ssh_command(
            connection,
            f"sudo cp {LOCAL_TRIDENT_CONFIG_PATH} {trident_config}",
        )

    trident_config_output = run_ssh_command(
        connection, f"sudo cat {trident_config}"
    ).strip()
    print("Trident configuration:\n", trident_config_output)


def trigger_rebuild_raid(
    ip_address,
    user_name,
    keys_file_path,
    trident_config,
):
    """Connects to the host via SSH, copies the Trident config to the host, and runs Trident rebuild-raid.

    Args:
        ip_address : The IP address of the host.
        user_name : The user name to ssh into the host with.
        keys_file_path : The full path to the file containing the host ssh keys.
        trident_config : The full path to the Trident config on the host.
    """
    # Set up SSH client
    config = Config(overrides={"connect_kwargs": {"key_filename": keys_file_path}})
    connection = Connection(host=ip_address, user=user_name, config=config)

    # Copy the Trident config to the host
    copy_host_config(connection, trident_config)

    # Re-build Trident and capture logs
    print("Re-building Trident", flush=True)
    trident_rebuild_raid(connection, trident_config)
    connection.close()


def main():
    # Setting argument_default=argparse.SUPPRESS means that the program will
    # halt attribute creation if no values provided for arg-s
    parser = argparse.ArgumentParser(
        allow_abbrev=True, argument_default=argparse.SUPPRESS
    )
    parser.add_argument(
        "-i",
        "--ip-address",
        type=str,
        help="IP address of the host.",
    )
    parser.add_argument(
        "-u",
        "--user-name",
        type=str,
        help="User name to ssh into the host with.",
    )
    parser.add_argument(
        "-k",
        "--keys-file-path",
        type=str,
        help="Full path to the file containing the host ssh keys.",
    )

    parser.add_argument(
        "-c",
        "--trident-config",
        type=str,
        help="File name of the custom read-write Trident config on the host to point Trident to.",
    )

    args = parser.parse_args()

    # Call helper func that runs Trident rebuild-raid
    trigger_rebuild_raid(
        args.ip_address,
        args.user_name,
        args.keys_file_path,
        args.trident_config,
    )


if __name__ == "__main__":
    main()
