#!/usr/bin/env python3

import argparse
import base64
from pathlib import Path

import yaml

# The installed interface is already in the SELinux development include path.
POLICY_FILES = ("trident.te", "trident.fc")
POLICY_SCRIPT_NAME = "load-candidate-trident-selinux-policy"
POLICY_INSTALL_SCRIPT_NAME = "install-candidate-trident-selinux-policy"
POLICY_STAGING_DIR = "/tmp/trident-cloud-agent-policy"
POLICY_PACKAGE_PATH = f"{POLICY_STAGING_DIR}/trident.pp"
TARGET_POLICY_PACKAGE_PATH = "/var/lib/trident/cloud-agent/trident.pp"


def build_policy_loader(policy_dir: Path) -> str:
    missing = [name for name in POLICY_FILES if not (policy_dir / name).is_file()]
    if missing:
        raise ValueError(
            f"Missing candidate SELinux policy files: {', '.join(missing)}"
        )

    commands = [
        "set -eux",
        f'policy_dir="{POLICY_STAGING_DIR}"',
        'rm -rf "$policy_dir"',
        'install -d -m 0700 "$policy_dir"',
    ]
    for name in POLICY_FILES:
        encoded = (
            base64.encodebytes((policy_dir / name).read_bytes())
            .decode("ascii")
            .rstrip()
        )
        marker = f"TRIDENT_{name.replace('.', '_').upper()}"
        commands.extend(
            [
                f"base64 --decode >\"$policy_dir/{name}\" <<'{marker}'",
                encoded,
                marker,
            ]
        )
    commands.extend(
        [
            'make -C "$policy_dir" -f /usr/share/selinux/devel/Makefile trident.pp',
            'semodule -i "$policy_dir/trident.pp"',
        ]
    )
    return "\n".join(commands) + "\n"


def build_target_policy_installer() -> str:
    return (
        "\n".join(
            [
                "set -eux",
                f'source_policy="{POLICY_PACKAGE_PATH}"',
                'if [ ! -f "$source_policy" ]; then',
                f'    source_policy="{TARGET_POLICY_PACKAGE_PATH}"',
                "fi",
                'test -f "$source_policy"',
                f'target_policy="$TARGET_ROOT{TARGET_POLICY_PACKAGE_PATH}"',
                'install -d -m 0700 "$(dirname "$target_policy")"',
                'if [ ! "$source_policy" -ef "$target_policy" ]; then',
                '    install -m 0600 "$source_policy" "$target_policy"',
                "fi",
                'semodule -p "$TARGET_ROOT" -i "$source_policy"',
                f'if [ "$source_policy" = "{POLICY_PACKAGE_PATH}" ]; then',
                f'    rm -rf "{POLICY_STAGING_DIR}"',
                "fi",
            ]
        )
        + "\n"
    )


def prepare_host_config(config: dict, policy_dir: Path) -> dict:
    config.setdefault("internalParams", {})["selfUpgradeTrident"] = True

    scripts = config.setdefault("scripts", {})
    pre_servicing = scripts.setdefault("preServicing", [])
    pre_servicing[:] = [
        script for script in pre_servicing if script.get("name") != POLICY_SCRIPT_NAME
    ]
    pre_servicing.insert(
        0,
        {
            "name": POLICY_SCRIPT_NAME,
            "runOn": ["clean-install"],
            "content": build_policy_loader(policy_dir),
        },
    )
    post_provision = scripts.setdefault("postProvision", [])
    post_provision[:] = [
        script
        for script in post_provision
        if script.get("name") != POLICY_INSTALL_SCRIPT_NAME
    ]
    post_provision.insert(
        0,
        {
            "name": POLICY_INSTALL_SCRIPT_NAME,
            "runOn": ["clean-install", "ab-update"],
            "content": build_target_policy_installer(),
        },
    )
    return config


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Add candidate-code overrides to a C1 Host Configuration."
    )
    parser.add_argument("host_config", type=Path)
    parser.add_argument("policy_dir", type=Path)
    args = parser.parse_args()

    config = yaml.safe_load(args.host_config.read_text(encoding="utf-8"))
    if not isinstance(config, dict):
        raise ValueError("Host Configuration root must be a mapping")

    prepare_host_config(config, args.policy_dir)
    args.host_config.write_text(
        yaml.safe_dump(config, sort_keys=False),
        encoding="utf-8",
    )


if __name__ == "__main__":
    main()
