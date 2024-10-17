#!/usr/bin/env python3
import argparse
import time
from typing import Dict, List, Tuple
from fabric import Connection, Config
import yaml
from invoke.exceptions import CommandTimedOut
from ssh_utilities import (
    run_ssh_command,
    OutputWatcher,
    TRIDENT_EXECUTABLE_PATH,
    EXECUTE_TRIDENT_CONTAINER,
    LOCAL_TRIDENT_CONFIG_PATH,
)
from io import StringIO

RETRY_INTERVAL = 60
MAX_RETRIES = 5


class YamlSafeLoader(yaml.SafeLoader):
    def accept_image(self, node):
        return self.construct_mapping(node)


def trident_run(
    connection, keys_file_path, ip_address, user_name, runtime_env, trident_config
):
    """
    Runs "trident run" to trigger A/B update on the host and ensure that the
    host completed staging or staging and finalizing of A/B update successfully.
    """
    # Initialize a watcher to return output live
    watcher = OutputWatcher()
    # Initialize stdout and stderr streams
    out_stream = StringIO()
    err_stream = StringIO()
    trident_command = (
        TRIDENT_EXECUTABLE_PATH if runtime_env == "host" else EXECUTE_TRIDENT_CONTAINER
    )
    try:
        # Set warn=True to continue execution even if the command has a non-zero exit code.
        # Provide -c arg, the full path to the RW Trident config.
        result = connection.run(
            f"sudo {trident_command} run -v trace -c {trident_config}",
            warn=True,
            out_stream=out_stream,
            err_stream=err_stream,
            timeout=240,
            watchers=[watcher],
        )

    except CommandTimedOut as timeout_exception:
        output = out_stream.getvalue() + err_stream.getvalue()
        # Handle the case where the command times out
        output_lines = output.strip().split("\n")
        if "[INFO  trident::engine] Rebooting system" in output_lines:
            print("Timeout occurred. Host rebooted successfully.")
            return
        else:
            raise Exception("Trident run timed out") from timeout_exception

    except Exception as e:
        print(f"An unexpected error occurred:\n")
        raise

    finally:
        connection.close()

    output = result.stdout + result.stderr
    output_lines = output.strip().split("\n")

    # Check the exit code: if 0, staging of A/B update succeeded.
    if (
        result.return_code == 0
        and "[INFO  trident::engine] Staging of update 'AbUpdate' succeeded"
        in output_lines
    ):
        print(
            "Received expected output with exit code 0. Staging of A/B update succeeded."
        )
    # If exit code is -1, host rebooted.
    elif (
        result.return_code == -1
        and "[INFO  trident::engine] Rebooting system" in output_lines
    ):
        print("Received expected output with exit code -1. Host rebooted successfully.")
    # If exit code is non 0 but host was running the rerun script, keep reconnecting.
    elif (
        result.return_code != 0
        and "[DEBUG trident::engine::hooks] Running script fail-on-the-first-run-to-force-rerun with interpreter /usr/bin/python3"
        in output_lines
    ):
        print("Detected an intentional Trident run failure. Attempting to reconnect...")
        for attempt in range(MAX_RETRIES):
            try:
                time.sleep(RETRY_INTERVAL)

                # Re-establish connection
                print(f"Attempt {attempt + 1}: Reconnecting to the host...")

                config = Config(
                    overrides={"connect_kwargs": {"key_filename": keys_file_path}}
                )
                connection = Connection(host=ip_address, user=user_name, config=config)

                # Check if the host is reachable
                run_ssh_command(
                    connection,
                    "sudo echo 'Successfully reconnected after A/B update'",
                )
                break
            except Exception as e:
                print(f"Reconnection attempt {attempt + 1} failed with exception: {e}")
            finally:
                connection.close()
        else:
            raise Exception("Maximum reconnection attempts exceeded.")
    else:
        raise Exception(
            f"Command unexpectedly returned with exit code {result.return_code} and output {output}"
        )

    # Return
    return


def update_host_config_images(
    connection, runtime_env, destination_directory, trident_config, version
):
    """Updates the images in the host configuration in the RW Trident config via sed command."""
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
    print("Original Trident configuration:\n", trident_config_output)

    # Get a list of images to be updated
    images_to_update = get_images_to_update(trident_config_output)
    host_directory = "/" if runtime_env == "host" else "/host/"
    destination_directory = destination_directory.strip("/")

    for old_url, image_name in images_to_update:
        new_url = f"file://{host_directory}{destination_directory}/{image_name}_v{version}.rawzst"
        print(f"Updating URL {old_url} to new URL {new_url}")
        sed_command = f"sudo sed -i 's#{old_url}#{new_url}#g' {trident_config}"
        run_ssh_command(connection, sed_command)

    # Remove selfUpgrade field from the Trident config
    run_ssh_command(
        connection,
        f"sudo sed -i 's#selfUpgrade: true#selfUpgrade: false#g' {trident_config}",
    )

    updated_trident_config_output = run_ssh_command(
        connection, f"sudo cat {trident_config}"
    ).strip()
    print("Updated Trident configuration:\n", updated_trident_config_output)


def get_images_to_update(trident_config_output) -> List[Tuple[str, str]]:
    """
    Based on the local Trident config, returns a list of tuples (URL, image_name), representing
    images that need to be updated. An image can only be updated if it corresponds to (1) the ESP
    partition or (2) an A/B volume pair.

    E.g. If the images section includes URL http://NETLAUNCH_HOST_ADDRESS/files/esp.rawzst for the
    ESP partition and URL http://NETLAUNCH_HOST_ADDRESS/files/root.rawzst for the root A/B volume
    pair, the function will return the following list:
    [
        (http://NETLAUNCH_HOST_ADDRESS/files/esp.rawzst, 'esp'),
        (http://NETLAUNCH_HOST_ADDRESS/files/root.rawzst, 'root')
    ].
    """
    YamlSafeLoader.add_constructor("!image", YamlSafeLoader.accept_image)
    trident_config = yaml.load(trident_config_output, Loader=YamlSafeLoader)

    # Determine targetId of ESP partition
    esp_partition_target_id = None
    for disk in trident_config["hostConfiguration"]["storage"]["disks"]:
        for partition in disk["partitions"]:
            if partition["type"] == "esp":
                esp_partition_target_id = partition["id"]
                break

    # Collect IDs of A/B volume pairs
    ab_volume_pair_ids = []
    for pair in trident_config["hostConfiguration"]["storage"]["abUpdate"][
        "volumePairs"
    ]:
        ab_volume_pair_ids.append(pair["id"])

    # Collect list of all devices with images: (image URL, deviceId)
    all_images: List[Tuple[str, str]] = []

    # First check all filesystems
    for fs in trident_config["hostConfiguration"]["storage"].get("filesystems") or []:
        device_id = fs.get("deviceId")
        if not device_id:
            continue
        source: Dict = fs.get("source", {})
        if source.get("type") in ["image", "esp-image"]:
            all_images.append((source["url"], device_id))

    # Then check all verity filesystems:
    for fs in (
        trident_config["hostConfiguration"]["storage"].get("verityFilesystems") or []
    ):
        all_images.append((fs["dataImage"]["url"], fs["dataDeviceId"]))
        all_images.append((fs["hashImage"]["url"], fs["hashDeviceId"]))

    # Collect URLs of images that can be updated
    urls_to_update = []
    for url, device_id in all_images:
        if device_id == esp_partition_target_id or device_id in ab_volume_pair_ids:
            if url.startswith("http://"):
                # Extract part between the last '/' and '.rawzst'
                image_name = url.split("/")[-1].split(".rawzst")[0]
            else:
                # Additional logic to strip "_vN", e.g. for URLs like file:///abupdate/esp_v2.rawzst,
                # extract 'esp'; for file:///run/verity_roothash_v2.rawzst, extract 'verity_roothash'
                image_name = "_".join(
                    url.split("/")[-1].split(".rawzst")[0].rsplit("_", 1)[:-1]
                )
            urls_to_update.append((url, image_name))

    return urls_to_update


def update_allowed_operations(
    connection, trident_config, stage_ab_update, finalize_ab_update
):
    """
    Updates the allowed operations in the Trident config to stage and/or finalize the A/B update.
    """
    # Determine the allowed operations to set
    allowed_operations = []
    if stage_ab_update:
        print("Staging of A/B update requested. Adding 'stage' to allowed operations.")
        allowed_operations.append("stage")
    if finalize_ab_update:
        print(
            "Finalizing of A/B update requested. Adding 'finalize' to allowed operations."
        )
        allowed_operations.append("finalize")

    # Create the allowed operations string for the sed command
    allowed_operations_str = "\\n".join(f"- {op}" for op in allowed_operations)
    # Construct the sed command to update the allowed operations
    sed_command = f"sudo sed -i '/allowedOperations:/,/hostConfiguration:/c\\allowedOperations:\\n{allowed_operations_str}\\nhostConfiguration:' {trident_config}"

    # Run the sed command to update the configuration
    run_ssh_command(connection, sed_command)

    # Print out updated Trident configuration
    updated_host_config_output = run_ssh_command(
        connection, f"sudo cat {trident_config}"
    ).strip()
    print("Updated allowed operations in Trident config:\n", updated_host_config_output)


def add_logstream(connection, trident_config):
    """
    Adds a logstream to the host configuration in the Trident config via sed command.
    """
    # Grab the current Trident config from the host
    trident_config_output = run_ssh_command(
        connection, f"sudo cat {trident_config}"
    ).strip()
    # Grab the phonehome string from the current Trident config
    trident_config_dict = yaml.load(trident_config_output, Loader=yaml.SafeLoader)
    # Check if logstream already exists
    if trident_config_dict.get("logstream") is None:
        phonehome = trident_config_dict["phonehome"]
        logstream_url = phonehome[:].replace("phonehome", "logstream")
        # Create the logstream string to add to the Trident config
        logstream_str = f"logstream: {logstream_url}"
        # Insert the logstream entry after the phonehome entry in the Trident config
        sed_command = (
            f"sudo sed -i '0,/phonehome:/a\\{logstream_str}\n' {trident_config}"
        )
        run_ssh_command(connection, sed_command)
        print("Added logstream in Trident config:\n")

        updated_host_config_output = run_ssh_command(
            connection, f"sudo cat {trident_config}"
        ).strip()
        print(updated_host_config_output)
    else:
        print("Logstream already exists in Trident config.")


def trigger_ab_update(
    ip_address,
    user_name,
    keys_file_path,
    destination_directory,
    trident_config,
    version,
    runtime_env,
    stage_ab_update,
    finalize_ab_update,
):
    """Connects to the host via SSH, updates the local Trident config, and re-runs Trident"""
    # Set up SSH client
    config = Config(overrides={"connect_kwargs": {"key_filename": keys_file_path}})
    connection = Connection(host=ip_address, user=user_name, config=config)

    # If staging of A/B update is required, update the images
    if stage_ab_update:
        update_host_config_images(
            connection, runtime_env, destination_directory, trident_config, version
        )

    # Update allowed operations in the Trident config
    update_allowed_operations(
        connection,
        trident_config,
        stage_ab_update,
        finalize_ab_update,
    )

    # Add logstream to the Trident config to stream with netlisten
    add_logstream(connection, trident_config)

    # Re-run Trident and capture logs
    print("Re-running Trident to trigger A/B update", flush=True)
    trident_run(
        connection, keys_file_path, ip_address, user_name, runtime_env, trident_config
    )
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
        "-d",
        "--destination-directory",
        type=str,
        help="Read-write directory on the host that contains the runtime OS images for the A/B update.",
    )
    parser.add_argument(
        "-c",
        "--trident-config",
        type=str,
        help="File name of the custom read-write Trident config on the host to point Trident to.",
    )
    parser.add_argument(
        "-v",
        "--version",
        type=str,
        help="Version of runtime OS images for the A/B update.",
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
        "-s",
        "--stage-ab-update",
        action="store_true",
        help="Controls whether A/B update should be staged.",
    )
    parser.add_argument(
        "-f",
        "--finalize-ab-update",
        action="store_true",
        help="Controls whether A/B update should be finalized.",
    )

    args = parser.parse_args()

    # Set to False if flags not provided
    stage_ab_update = getattr(args, "stage_ab_update", False)
    finalize_ab_update = getattr(args, "finalize_ab_update", False)

    # Call helper func that triggers A/B update
    trigger_ab_update(
        args.ip_address,
        args.user_name,
        args.keys_file_path,
        args.destination_directory,
        args.trident_config,
        args.version,
        args.runtime_env,
        stage_ab_update,
        finalize_ab_update,
    )


if __name__ == "__main__":
    main()
