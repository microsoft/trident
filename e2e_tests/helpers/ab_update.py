#!/usr/bin/env python3
import argparse
import time
from typing import Dict, List, Tuple
import yaml
from invoke.exceptions import CommandTimedOut
from ssh_utilities import (
    check_file_exists,
    create_ssh_connection,
    LOCAL_TRIDENT_CONFIG_PATH,
    run_ssh_command,
    trident_run,
)

RETRY_INTERVAL = 60
MAX_RETRIES = 5


class YamlSafeLoader(yaml.SafeLoader):
    def accept_image(self, node):
        return self.construct_mapping(node)


def trident_run_command(
    connection, keys_file_path, ip_address, user_name, runtime_env, trident_config
):
    """
    Runs "trident run" to trigger A/B update on the host and ensure that the
    host completed staging or staging and finalizing of A/B update successfully.
    """
    try:
        # Provide -c arg, the full path to the RW Trident config.
        trident_return_code, trident_output = trident_run(
            connection, f"run -v trace -c {trident_config}", runtime_env
        )
    except CommandTimedOut as timeout_exception:
        # Access the output from the exception and look for reboot information in Trident output.
        timeout_trident_output = "".join(timeout_exception.args)
        trident_output_lines = timeout_trident_output.strip().split("\n")
        if "[INFO  trident::engine] Rebooting system" in trident_output_lines:
            print("Host rebooted successfully. Timeout occurred.")
            return
        else:
            raise Exception("Trident run timed out") from timeout_exception

    # Check the exit code: if 0, staging of A/B update succeeded.
    trident_output_lines = trident_output.strip().split("\n")
    if (
        trident_return_code == 0
        and "[INFO  trident::engine] Staging of update 'AbUpdate' succeeded"
        in trident_output_lines
    ):
        print(
            "Received expected output with exit code 0. Staging of A/B update succeeded."
        )
    # If exit code is -1, host rebooted.
    elif (
        trident_return_code == -1
        and "[INFO  trident::engine] Rebooting system" in trident_output_lines
    ):
        print("Received expected output with exit code -1. Host rebooted successfully.")
    # If exit code is non 0 but host was running the rerun script, keep reconnecting.
    elif (
        trident_return_code != 0
        and "[DEBUG trident::engine::hooks] Running script fail-on-the-first-run-to-force-rerun with interpreter /usr/bin/python3"
        in trident_output_lines
    ):
        print("Detected an intentional Trident run failure. Attempting to reconnect...")
        for attempt in range(MAX_RETRIES):
            try:
                time.sleep(RETRY_INTERVAL)

                # Re-establish connection
                print(f"Attempt {attempt + 1}: Reconnecting to the host...")

                connection = create_ssh_connection(
                    ip_address, user_name, keys_file_path
                )

                # Check if the host is reachable
                run_ssh_command(
                    connection, "echo 'Successfully reconnected after A/B update'"
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
            f"Command unexpectedly returned with exit code {trident_return_code} and output {output}"
        )

    # Return
    return


def update_host_config_images(
    connection, runtime_env, destination_directory, trident_config, version
):
    """Updates the images in the host configuration in the RW Trident config via sed command."""
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
    print("Original Trident configuration:\n", trident_config_output)

    # Get a list of images to be updated
    images_to_update = get_images_to_update(trident_config_output)
    host_directory = "/" if runtime_env == "host" else "/host/"
    destination_directory = destination_directory.strip("/")

    for old_url, image_name in images_to_update:
        new_url = f"file://{host_directory}{destination_directory}/{image_name}_v{version}.rawzst"
        print(f"Updating URL {old_url} to new URL {new_url}")
        sed_command = f"sed -i 's#{old_url}#{new_url}#g' {trident_config}"
        run_ssh_command(connection, sed_command, use_sudo=True)

    # Remove selfUpgrade field from the Trident config
    run_ssh_command(
        connection,
        f"sed -i 's#selfUpgrade: true#selfUpgrade: false#g' {trident_config}",
        use_sudo=True,
    )

    updated_trident_config_output = run_ssh_command(
        connection,
        f"cat {trident_config}",
        use_sudo=True,
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
    sed_command = f"sed -i '/allowedOperations:/,/hostConfiguration:/c\\allowedOperations:\\n{allowed_operations_str}\\nhostConfiguration:' {trident_config}"

    # Run the sed command to update the configuration
    run_ssh_command(connection, sed_command, use_sudo=True)

    # Print out updated Trident configuration
    updated_host_config_output = run_ssh_command(
        connection,
        f"cat {trident_config}",
        use_sudo=True,
    ).strip()
    print("Updated allowed operations in Trident config:\n", updated_host_config_output)


def add_logstream(connection, trident_config):
    """
    Adds a logstream to the host configuration in the Trident config via sed command.
    """
    # Grab the current Trident config from the host
    trident_config_output = run_ssh_command(
        connection,
        f"cat {trident_config}",
        use_sudo=True,
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
        sed_command = f"sed -i '0,/phonehome:/a\\{logstream_str}\n' {trident_config}"
        run_ssh_command(connection, sed_command)
        print("Added logstream in Trident config:\n")

        updated_host_config_output = run_ssh_command(
            connection,
            f"cat {trident_config}",
            use_sudo=True,
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
    connection = create_ssh_connection(ip_address, user_name, keys_file_path)

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
    trident_run_command(
        connection, keys_file_path, ip_address, user_name, runtime_env, trident_config
    )
    connection.close()

    # For container testing, manually trigger Trident run on the updated Runtime OS.
    if runtime_env == "container":
        for attempt in range(MAX_RETRIES):
            try:
                time.sleep(RETRY_INTERVAL)
                # Re-establish connection
                print(f"Attempt {attempt + 1}: Run Trident after A/B update")

                connection = create_ssh_connection(
                    ip_address, user_name, keys_file_path
                )

                trident_return_code, trident_output = trident_run(
                    connection, f"run", runtime_env
                )

                if trident_return_code == 0:
                    break
                else:
                    raise Exception(trident_output)
            except Exception as e:
                print(f"Trident run attempt {attempt + 1} failed: {e}")
            finally:
                connection.close()
        else:
            raise Exception(
                "Maximum attempts exceeded. Trident was unable to successfully run after A/B update."
            )

    return


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
