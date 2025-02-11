#!/usr/bin/env python3
import argparse
import io
import time
from typing import Dict, List, Tuple
import yaml
import logging
from invoke.exceptions import CommandTimedOut
from ssh_utilities import (
    LOCAL_TRIDENT_CONFIG_PATH,
    create_ssh_connection,
    run_ssh_command,
    trident_run,
)

RETRY_INTERVAL = 60
MAX_RETRIES = 5

logging.basicConfig(level=logging.INFO)
log = logging.getLogger("ab_update")


class YamlSafeLoader(yaml.SafeLoader):
    def accept_image(self, node):
        return self.construct_mapping(node)


def trident_run_command(
    connection,
    runtime_env,
    stage_ab_update,
    finalize_ab_update,
    trident_config_path,
):
    """
    Composes and runs a command to trigger A/B update on the host and processes a range of
    possible outcomes.
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

    for attempt in range(MAX_RETRIES):
        try:
            print(f"Attempt {attempt + 1}: Running Trident run command...")

            # Provide -c arg, the full path to the RW Trident config.
            trident_return_code, trident_stdout, trident_stderr = trident_run(
                connection,
                f"run -v trace -c {trident_config_path} --allowed-operations {allowed_operations_str}",
                runtime_env,
            )

            if (
                trident_return_code == 0
                and "Staging of update 'AbUpdate' succeeded" in trident_stderr
            ):
                print("Staging of A/B update succeeded")
                return
            elif trident_return_code == -1 and "Rebooting system" in trident_stderr:
                print("Host rebooted successfully")
                return
            elif trident_return_code == 2 and (
                "Failed to run post-configure script 'fail-on-the-first-run'"
                in trident_stderr
            ):
                print("Detected intentional failure. Re-running...")
                continue
            else:
                raise Exception(
                    f"Unexpected exit code {trident_return_code}. Output: {trident_stdout + trident_stderr}"
                )
        except Exception as e:
            print(f"Exception during Trident run: {e}")
            raise
    raise Exception("Maximum retries exceeded for Trident run")


def update_osimage_url(runtime_env, destination_directory, host_config, version):
    """Updates the OS image URL in Host Configuration."""

    host_directory = "/" if runtime_env == "host" else "/host/"
    destination_directory = destination_directory.strip("/")

    old_url = host_config.get("osImage", {}).get("url", None)
    if old_url is None:
        raise Exception("No current osImage URL found in Host Configuration")

    if old_url.startswith("http://"):
        # Extract part between the last '/' and '.cosi'
        image_name = old_url.split("/")[-1].split(".cosi")[0]
    else:
        # Additional logic to strip "_vN", e.g. for URLs like file:///abupdate/regular_v2.cosi,
        # extract 'regular'; for file:///run/verity_v2.cosi, extract 'verity'
        image_name = "_".join(
            old_url.split("/")[-1].split(".cosi")[0].rsplit("_", 1)[:-1]
        )
    new_url = (
        f"file://{host_directory}{destination_directory}/{image_name}_v{version}.cosi"
    )

    log.info(f"Updating OS image URL from {old_url} to {new_url}")
    host_config["osImage"]["url"] = new_url

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

    # If staging of A/B update is required, update the OS image URL in Host Configuration
    if stage_ab_update:
        update_osimage_url(runtime_env, destination_directory, host_config, version)
        connection.run("sudo mkdir -p /tmp/staging")
        connection.run("sudo chmod 777 /tmp/staging")
        connection.put(io.StringIO(yaml.dump(host_config)), "/tmp/staging/hc.yaml")
        connection.run(f"sudo mv /tmp/staging/hc.yaml {trident_config_path}")

    # Re-run Trident and capture logs
    print("Re-running Trident to trigger A/B update", flush=True)
    trident_run_command(
        connection,
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
