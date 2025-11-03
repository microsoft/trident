import argparse
import os
import random
import yaml


def inject_uefi_fallback_testing(host_config_path, runtimeEnv, uefiFallbackMode=None):
    with open(host_config_path, "r") as f:
        host_config = yaml.safe_load(f)

    if "os" not in host_config:
        host_config["os"] = {}
    # Only inject testing values if uefiFallback
    # is not already set.
    if not hasattr(host_config["os"], "uefiFallback"):
        if uefiFallbackMode is None:
            uefi_fallback_modes = ["none", "rollback", "rollforward"]
            # Randomly pick a fallback mode for testing.
            uefiFallbackMode = random.choice(uefi_fallback_modes)
        host_config["os"]["uefiFallback"] = uefiFallbackMode

        # Get uefi fallback health check script content
        health_check_script_path = (
            os.path.dirname(os.path.abspath(__file__))
            + "/uefi_fallback_validation_script.txt"
        )
        with open(health_check_script_path, "r") as f:
            health_check_content = f.read()

        # Replace placeholder with selected uefi fallback mode
        health_check_content = health_check_content.replace(
            "_REPLACE_FALLBACK_NODE_", uefiFallbackMode
        )
        root_path = ""
        if runtimeEnv == "container":
            root_path = "/host"
        health_check_content = health_check_content.replace(
            "_REPLACE_ROOT_PREFIX_", root_path
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
        "--uefiFallbackMode",
        type=str,
        required=False,
        help="UEFI fallback mode to inject.",
    )
    parser.add_argument(
        "-r",
        "--runtimeEnv",
        type=str,
        required=True,
        choices=["host", "container"],
        help="The runtime environment of Trident (e.g., host or container).",
    )
    args = parser.parse_args()

    inject_uefi_fallback_testing(
        args.hostconfig, args.runtimeEnv, args.uefiFallbackMode
    )


if __name__ == "__main__":
    main()
