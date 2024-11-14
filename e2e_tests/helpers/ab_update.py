#!/usr/bin/env python3
import argparse
import io
import time
from typing import Dict, List, Tuple
import yaml
from invoke.exceptions import CommandTimedOut
from ssh_utilities import (
    LOCAL_TRIDENT_CONFIG_PATH,
    create_ssh_connection,
    run_ssh_command,
    trident_run,
)

RETRY_INTERVAL = 60
MAX_RETRIES = 5


class YamlSafeLoader(yaml.SafeLoader):
    def accept_image(self, node):
        return self.construct_mapping(node)


def trident_run_command(
    connection,
    keys_file_path,
    ip_address,
    user_name,
    runtime_env,
    stage_ab_update,
    finalize_ab_update,
    trident_config_path,
):
    """
    Runs "trident run" to trigger A/B update on the host and ensure that the
    host completed staging or staging and finalizing of A/B update successfully.
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
    allowed_operations_str = ",".join(allowed_operations)

    try:
        # Provide -c arg, the full path to the RW Trident config.
        trident_return_code, trident_stdout, trident_stderr = trident_run(
            connection,
            f"run -v trace -c {trident_config_path} --allowed-operations {allowed_operations_str}",
            runtime_env,
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
    trident_output_lines = trident_stderr.strip().split("\n")
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
            f"Command unexpectedly returned with exit code {trident_return_code} and output {trident_stdout + trident_stderr}"
        )

    # Return
    return


def update_host_config_images(runtime_env, destination_directory, host_config, version):
    """Updates the images in the host configuration in the RW Trident config via sed command."""

    host_directory = "/" if runtime_env == "host" else "/host/"
    destination_directory = destination_directory.strip("/")

    # Determine targetId of ESP partition
    esp_partition_target_id = None
    for disk in host_config["storage"]["disks"]:
        for partition in disk["partitions"]:
            if partition["type"] == "esp":
                esp_partition_target_id = partition["id"]
                break

    # Collect IDs of A/B volume pairs
    ab_volume_pair_ids = []
    for pair in host_config["storage"]["abUpdate"]["volumePairs"]:
        ab_volume_pair_ids.append(pair["id"])

    def update_url(device_id, old_url):
        if device_id == esp_partition_target_id or device_id in ab_volume_pair_ids:
            if old_url.startswith("http://"):
                # Extract part between the last '/' and '.rawzst'
                image_name = old_url.split("/")[-1].split(".rawzst")[0]
            else:
                # Additional logic to strip "_vN", e.g. for URLs like file:///abupdate/esp_v2.rawzst,
                # extract 'esp'; for file:///run/verity_roothash_v2.rawzst, extract 'verity_roothash'
                image_name = "_".join(
                    old_url.split("/")[-1].split(".rawzst")[0].rsplit("_", 1)[:-1]
                )
            return f"file://{host_directory}{destination_directory}/{image_name}_v{version}.rawzst"
        else:
            return old_url

    # First check all filesystems
    for fs in host_config["storage"].get("filesystems") or []:
        device_id = fs.get("deviceId")
        if not device_id:
            continue
        source: Dict = fs.get("source", {})
        if source.get("type") in ["image", "esp-image"]:
            source["url"] = update_url(device_id, source["url"])

    # Then check all verity filesystems:
    for fs in host_config["storage"].get("verityFilesystems") or []:
        fs["dataImage"]["url"] = update_url(fs["dataDeviceId"], fs["dataImage"]["url"])
        fs["hashImage"]["url"] = update_url(fs["hashDeviceId"], fs["hashImage"]["url"])

    # Remove selfUpgrade field from the Trident config
    host_config["trident"]["selfUpgrade"] = False


def trigger_ab_update(
    ip_address,
    user_name,
    keys_file_path,
    destination_directory,
    trident_config_path,
    version,
    runtime_env,
    stage_ab_update,
    finalize_ab_update,
):
    """Connects to the host via SSH, updates the local Trident config, and re-runs Trident"""
    # Set up SSH client
    connection = create_ssh_connection(ip_address, user_name, keys_file_path)

    _, trident_stdout, _ = trident_run(connection, "get", runtime_env)
    host_status = trident_stdout.strip()

    YamlSafeLoader.add_constructor("!image", YamlSafeLoader.accept_image)
    host_status_dict = yaml.load(host_status, Loader=yaml.SafeLoader)
    host_config = host_status_dict["spec"]

    # If staging of A/B update is required, update the images
    if stage_ab_update:
        update_host_config_images(
            runtime_env, destination_directory, host_config, version
        )
        connection.run("sudo mkdir -p /tmp/staging")
        connection.run("sudo chmod 777 /tmp/staging")
        connection.put(io.StringIO(yaml.dump(host_config)), "/tmp/staging/hc.yaml")
        connection.run(f"sudo mv /tmp/staging/hc.yaml {trident_config_path}")

    # Re-run Trident and capture logs
    print("Re-running Trident to trigger A/B update", flush=True)
    trident_run_command(
        connection,
        keys_file_path,
        ip_address,
        user_name,
        runtime_env,
        stage_ab_update,
        finalize_ab_update,
        trident_config_path,
    )
    connection.close()

    # For container testing, finalize the A/B update by manually triggering
    # a Trident run on the updated Runtime OS.
    if finalize_ab_update and runtime_env == "container":
        for attempt in range(MAX_RETRIES):
            try:
                time.sleep(RETRY_INTERVAL)
                # Re-establish connection
                print(f"Attempt {attempt + 1}: Run Trident after A/B update")

                connection = create_ssh_connection(
                    ip_address, user_name, keys_file_path
                )

                trident_return_code, trident_stdout, trident_stderr = trident_run(
                    connection, f"run", runtime_env
                )

                if trident_return_code == 0:
                    break
                else:
                    raise Exception(trident_stdout + trident_stderr)
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
