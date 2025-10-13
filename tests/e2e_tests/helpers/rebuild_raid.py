#!/usr/bin/env python3
import argparse
from fabric import Connection, Config
from invoke.watchers import StreamWatcher
from io import StringIO
from ssh_utilities import (
    check_file_exists,
    create_ssh_connection,
    LOCAL_TRIDENT_CONFIG_PATH,
    run_ssh_command,
    trident_run,
)


def trident_rebuild_raid(connection, trident_config, runtime_env):
    """
    Runs "trident rebuild-raid" to trigger rebuilding RAID and checks if RAID was rebuilt successfully.

    Args:
        connection : The SSH connection to the host.
        trident_config : The full path to the Trident config on the host.

    """
    run_ssh_command(
        connection,
        f"sed -i 's|waitForSystemdNetworkd.*||' {trident_config}",
        use_sudo=True,
    )
    run_ssh_command(
        connection,
        f"sed -i 's|orchestratorConnectionTimeoutSeconds.*||' {trident_config}",
        use_sudo=True,
    )

    # Provide -c arg, the full path to the RW Trident config.
    trident_return_code, trident_stdout, trident_stderr = trident_run(
        connection, f"rebuild-raid -v trace", runtime_env
    )
    trident_output = trident_stdout + trident_stderr
    print("Trident rebuild-raid output {}".format(trident_output))

    # Check the exit code: if 0, Trident rebuild-raid succeeded.
    if trident_return_code == 0:
        print(
            "Received expected output with exit code 0. Trident rebuild-raid succeeded."
        )
    else:
        raise Exception(
            f"Command unexpectedly returned with exit code {trident_return_code} and output {trident_output}"
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
    if not check_file_exists(connection, trident_config):
        print(
            f"File {trident_config} does not exist. Copying from {LOCAL_TRIDENT_CONFIG_PATH}"
        )
        run_ssh_command(
            connection,
            f"cp {LOCAL_TRIDENT_CONFIG_PATH} {trident_config}",
            use_sudo=True,
        )

    trident_config_output = run_ssh_command(
        connection,
        f"cat {trident_config}",
        use_sudo=True,
    ).strip()
    print("Trident configuration:\n", trident_config_output)


def trigger_rebuild_raid(
    ip_address,
    user_name,
    keys_file_path,
    runtime_env,
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
    connection = create_ssh_connection(ip_address, user_name, keys_file_path)

    # Copy the Trident config to the host
    copy_host_config(connection, trident_config)

    # Re-build Trident and capture logs
    print("Re-building Trident", flush=True)
    trident_rebuild_raid(connection, trident_config, runtime_env)
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
        "-e",
        "--runtime-env",
        action="store",
        type=str,
        choices=["host", "container"],
        default="host",
        help="Runtime environment for trident: 'host' or 'container'. Default is 'host'.",
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
        args.runtime_env,
        args.trident_config,
    )


if __name__ == "__main__":
    main()
