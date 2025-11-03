import argparse
import os
import random
import yaml


def inject_uefi_fallback_testing(host_config_path, uefi_fallback_mode=None):
    with open(host_config_path, "r") as f:
        host_config = yaml.safe_load(f)

    if "os" not in host_config:
        host_config["os"] = {}
    # Only inject testing values if uefiFallback
    # is not already set.
    if not hasattr(host_config["os"], "uefiFallback"):
        if uefi_fallback_mode is None:
            uefi_fallback_modes = ["none", "rollback", "rollforward"]
            # Randomly pick a fallback mode for testing.
            uefi_fallback_mode = random.choice(uefi_fallback_modes)
        host_config["os"]["uefiFallback"] = uefi_fallback_mode

        # Get uefi fallback health check script content
        health_check_script_path = (
            os.path.dirname(os.path.abspath(__file__))
            + "/uefi_fallback_validation_script.txt"
        )
        with open(health_check_script_path, "r") as f:
            health_check_content = f.read()

        # Replace placeholder with selected uefi fallback mode
        health_check_content = health_check_content.replace(
            "_REPLACE_FALLBACK_NODE_", uefi_fallback_mode
        )

        if "health" not in host_config:
            host_config["health"] = {}
        if "checks" not in host_config["health"]:
            host_config["health"]["checks"] = []
        host_config["health"]["checks"].append(
            {
                "name": "uefi-fallback-validation-update",
                "content": health_check_content.replace(
                    "_REPLACE_NOT_INSTALL_", "true"
                ),
                "runOn": ["ab-update"],
            }
        )
        host_config["health"]["checks"].append(
            {
                "name": "uefi-fallback-validation-install",
                "content": health_check_content.replace(
                    "_REPLACE_NOT_INSTALL_", "false"
                ),
                "runOn": ["clean-install"],
            }
        )

    with open(host_config_path, "w") as f:
        yaml.safe_dump(host_config, f)


def main():
    parser = argparse.ArgumentParser(
        description="Modifies Host Configuration to inject uefiFallback and add health checks to validation."
    )
    parser.add_argument(
        "-t",
        "--hostconfig",
        type=str,
        required=True,
        help="Path to the Trident configuration file.",
    )
    parser.add_argument(
        "--uefi-fallback-mode",
        type=str,
        required=False,
        help="UEFI fallback mode to inject.",
    )
    args = parser.parse_args()

    inject_uefi_fallback_testing(args.hostconfig, args.uefi_fallback_mode)


if __name__ == "__main__":
    main()
