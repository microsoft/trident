import argparse
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
            random_mode = random.choice(uefi_fallback_modes)
            host_config["os"]["uefiFallback"] = random_mode
        else:
            host_config["os"]["uefiFallback"] = uefi_fallback_mode
        health_check_content = """EFI_PATH="/boot/efi/EFI"
FALLBACK_PATH="$EFI_PATH/BOOT"

EFI_OUTPUT="$(efibootmgr)"
echo "$EFI_OUTPUT"

CURRENT_BOOT="$(echo "$EFI_OUTPUT" | grep "BootCurrent")"
if [ -z "$CURRENT_BOOT" ]; then
    echo "Failed to get current boot entry"
    exit 1
fi

CURRENT_BOOT_ENTRY="$(echo "$CURRENT_BOOT" | awk '{print $2}'"
if [ -z "$CURRENT_BOOT_ENTRY" ]; then
    echo "Failed to parse current boot entry"
    exit 1
fi

CURRENT_AZL_BOOT_NAME="$(echo "$EFI_OUTPUT" | grep "Boot${CURRENT_BOOT_ENTRY}" | awk '{print $2}' | grep "AZL")"
if [ -z "$CURRENT_AZL_BOOT_NAME" ]; then
    echo "Current boot entry is not an AZL boot entry"
    exit 1
fi

if [ "_REPLACE_FALLBACK_NODE_" == "none" ]; then
    # if none, check that $FALLBACK_PATH is empty
    if sudo find $FALLBACK_PATH/*; then
        echo "$FALLBACK_PATH is not empty"
        exit 1
    else
        echo "$FALLBACK_PATH is empty"
        exit 0
    fi
else
    AZL_BOOT_NAME_TO_CHECK="$CURRENT_AZL_BOOT_NAME"
    if [ "_REPLACE_FALLBACK_NODE_" == "rollback" ] && _REPLACE_NOT_INSTALL_; then
        # if rollback, check that $FALLBACK_PATH == opposite of $EFI_PATH/$CURRENT_AZL_BOOT_NAME
        AZL_BOOT_NAME_TO_CHECK="$(echo $CURRENT_AZL_BOOT_NAME | sed "s/AZLA/AZLA_TMP/g; s/AZLB/AZLA/g; s/AZLA_TMP/AZLB/g")"
    fi

    if diff $FALLBACK_PATH/ $EFI_PATH/$AZL_BOOT_NAME_TO_CHECK/; then
        echo "no difference detected between $FALLBACK_PATH and $EFI_PATH/$AZL_BOOT_NAME_TO_CHECK/"
        exit 0
    else
        echo "difference detected between $FALLBACK_PATH and $EFI_PATH/$AZL_BOOT_NAME_TO_CHECK/"
        exit 1
    fi
fi
"""
        health_check_content = health_check_content.replace(
            "_REPLACE_FALLBACK_NODE_", random_mode
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
