#!/usr/bin/env python3
import argparse
import time
from fabric import Connection, Config
import yaml

HOST_TRIDENT_CONFIG_PATH = "/etc/trident/config.yaml"
TRIDENT_EXECUTABLE_PATH = "/usr/bin/trident"
RETRY_INTERVAL = 60
MAX_RETRIES = 5


class YamlSafeLoader(yaml.SafeLoader):
    def accept_image(self, node):
        return self.construct_mapping(node)


# Runs a command on the host using Fabric and returns the combined stdout and stderr
def run_ssh_command(connection, command):
    try:
        # Executes a command using Fabric and returns the result
        result = connection.run(command)
        # Combining stdout and stderr for compatibility with the original function's return
        return result.stdout + result.stderr
    except Exception as e:
        print(f"An error occurred: {e}")
        raise


# Runs "trident run" to trigger A/B update on the host and ensure that the host reached the reboot
# stage successfully
def trident_run(connection, keys_file_path, ip_address, user_name):
    try:
        # Set warn=True to continue execution even if the command has a non-zero exit code
        result = connection.run(
            f"sudo {TRIDENT_EXECUTABLE_PATH} run -v trace", warn=True
        )
        output_lines = (result.stdout + result.stderr).strip().split("\n")

        # Check the exit code: if -1, host rebooted
        if (
            result.return_code == -1
            and "[INFO  trident::modules] Rebooting system" in output_lines
        ):
            print("Received expected output with exit code -1. Host rebooted.")
        # If non-zero exit code but host was running the rerun script, keep reconnecting
        elif (
            result.return_code != 0
            and "[DEBUG trident::modules::hooks] Running script fail-on-the-first-run-to-force-rerun with interpreter /usr/bin/python3"
            in output_lines
        ):
            print(
                "Detected an intentional Trident run failure. Attempting to reconnect..."
            )
            connection.close()
            for attempt in range(MAX_RETRIES):
                try:
                    time.sleep(RETRY_INTERVAL)

                    # Re-establish connection
                    print(f"Attempt {attempt + 1}: Reconnecting to the host...")

                    config = Config(
                        overrides={"connect_kwargs": {"key_filename": keys_file_path}}
                    )
                    connection = Connection(
                        host=ip_address, user=user_name, config=config
                    )

                    # Check if the host is reachable
                    run_ssh_command(
                        connection,
                        "sudo echo 'Successfully reconnected after A/B update'",
                    )
                    connection.close()
                    break
                except Exception as e:
                    print(f"Reconnection attempt {attempt + 1} failed: {e}")
                    connection.close()
            else:
                raise Exception("Maximum reconnection attempts exceeded.")
        elif result.return_code != 0:
            connection.close()
            raise Exception(f"Command failed with exit code {result.return_code}")

        # Return combined stdout and stderr
        return result.stdout + result.stderr

    except Exception as e:
        print(f"An error occurred: {e}")
        connection.close()
        raise


# Update host's Trident config by editing /etc/trident/config.yaml via sed command
def update_host_trident_config(connection, image_dir, version):
    print("Original Trident configuration:")
    host_config_output = run_ssh_command(
        connection, f"sudo cat {HOST_TRIDENT_CONFIG_PATH}"
    ).strip()

    # Get a list of images to be updated
    images_to_update = get_images_to_update(host_config_output)
    image_dir = image_dir.strip("/")

    for old_url, image_name in images_to_update:
        new_url = f"file:///{image_dir}/{image_name}_v{version}.rawzst"
        print(f"Updating URL {old_url} to new URL {new_url}")
        sed_command = (
            f"sudo sed -i 's#{old_url}#{new_url}#g' {HOST_TRIDENT_CONFIG_PATH}"
        )
        run_ssh_command(connection, sed_command)

    # Remove phonehome and selfUpgrade fields from the Trident config
    run_ssh_command(
        connection, f"sudo sed -i -r 's#phonehome:.+##g' {HOST_TRIDENT_CONFIG_PATH}"
    )
    run_ssh_command(
        connection,
        f"sudo sed -i 's#selfUpgrade: true#selfUpgrade: false#g' {HOST_TRIDENT_CONFIG_PATH}",
    )

    print("Updated Trident configuration:")
    run_ssh_command(connection, f"sudo cat {HOST_TRIDENT_CONFIG_PATH}")


# Based on the local Trident config, returns a list of tuples (URL, image_name), representing
# images that need to be updated. An image can only be updated if it corresponds to (1) the ESP
# partition or (2) an A/B volume pair.
#
# E.g. If the images section includes URL http://NETLAUNCH_HOST_ADDRESS/files/esp.rawzst for the
# ESP partition and URL http://NETLAUNCH_HOST_ADDRESS/files/root.rawzst for the root A/B volume
# pair, the function will return the following list:
# [(http://NETLAUNCH_HOST_ADDRESS/files/esp.rawzst, 'esp'),
# (http://NETLAUNCH_HOST_ADDRESS/files/root.rawzst, 'root')].
def get_images_to_update(host_config_output):
    YamlSafeLoader.add_constructor("!image", YamlSafeLoader.accept_image)
    host_config = yaml.load(host_config_output, Loader=YamlSafeLoader)

    # Determine targetId of ESP partition
    esp_partition_target_id = None
    for disk in host_config["hostConfiguration"]["storage"]["disks"]:
        for partition in disk["partitions"]:
            if partition["type"] == "esp":
                esp_partition_target_id = partition["id"]
                break

    # Collect IDs of A/B volume pairs
    ab_volume_pair_ids = []
    for pair in host_config["hostConfiguration"]["storage"]["abUpdate"]["volumePairs"]:
        ab_volume_pair_ids.append(pair["id"])

    # Collect URLs of images that can be updated
    urls_to_update = []
    for image in host_config["hostConfiguration"]["storage"]["images"]:
        target_id = image["targetId"]
        if target_id == esp_partition_target_id or target_id in ab_volume_pair_ids:
            url = image["url"]
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


# Connects to the host via SSH, updates the local Trident config, and triggers A/B update
def trigger_ab_update(ip_address, user_name, keys_file_path, image_dir, version):
    # Set up SSH client
    config = Config(overrides={"connect_kwargs": {"key_filename": keys_file_path}})
    connection = Connection(host=ip_address, user=user_name, config=config)

    # Update host's Trident config
    update_host_trident_config(connection, image_dir, version)

    # Re-run Trident and capture logs
    print("Re-running Trident to trigger A/B update")
    trident_run(connection, keys_file_path, ip_address, user_name)
    connection.close()

    print("Going to sleep for 60 seconds")
    time.sleep(60)

    # Reconnect to the SSH server
    config = Config(overrides={"connect_kwargs": {"key_filename": keys_file_path}})
    connection = Connection(host=ip_address, user=user_name, config=config)

    run_ssh_command(connection, "sudo echo 'Successfully reconnected after A/B update'")

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
        "--image-dir",
        type=str,
        help="Directory on the host that contains the runtime OS images for the A/B update.",
    )
    parser.add_argument(
        "-v",
        "--version",
        type=str,
        help="Version of runtime OS images for the A/B update.",
    )

    args = parser.parse_args()

    # Call helper func that triggers A/B update
    trigger_ab_update(
        args.ip_address,
        args.user_name,
        args.keys_file_path,
        args.image_dir,
        args.version,
    )


if __name__ == "__main__":
    main()
