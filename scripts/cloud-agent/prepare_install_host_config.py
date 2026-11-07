#!/usr/bin/env python3

import argparse
import base64
from pathlib import Path

import yaml

POLICY_FILES = ("trident.te", "trident.if", "trident.fc")
POLICY_SCRIPT_NAME = "load-candidate-trident-selinux-policy"


def build_policy_loader(policy_dir: Path) -> str:
    missing = [name for name in POLICY_FILES if not (policy_dir / name).is_file()]
    if missing:
        raise ValueError(
            f"Missing candidate SELinux policy files: {', '.join(missing)}"
        )

    commands = [
        "set -eux",
        'policy_dir="$(mktemp -d /tmp/trident-policy.XXXXXX)"',
        "trap 'rm -rf \"$policy_dir\"' EXIT",
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
