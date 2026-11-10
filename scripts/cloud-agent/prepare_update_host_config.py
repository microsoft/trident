#!/usr/bin/env python3

import argparse
import re
import urllib.parse

from pathlib import Path, PurePosixPath

import yaml


def versioned_image_url(image_url: str, version: int) -> str:
    parsed = urllib.parse.urlsplit(image_url)
    if parsed.scheme not in ("http", "https") or not parsed.netloc:
        raise ValueError(f"Unsupported update image URL: {image_url}")

    path = PurePosixPath(parsed.path)
    match = re.fullmatch(r"(.*?)(?:_v\d+)?(\.[^.]+)", path.name)
    if not match:
        raise ValueError(f"Cannot version update image URL: {image_url}")

    versioned_name = f"{match.group(1)}_v{version}{match.group(2)}"
    versioned_path = str(path.with_name(versioned_name))
    return urllib.parse.urlunsplit(
        (parsed.scheme, parsed.netloc, versioned_path, parsed.query, parsed.fragment)
    )


def prepare_host_config(config: dict, version: int) -> str:
    image = config.get("image")
    if not isinstance(image, dict) or not isinstance(image.get("url"), str):
        raise ValueError("Host Configuration image.url must be a string")

    image["url"] = versioned_image_url(image["url"], version)
    image["sha384"] = "ignored"
    internal_params = config.get("internalParams")
    if internal_params is None:
        internal_params = {}
        config["internalParams"] = internal_params
    if not isinstance(internal_params, dict):
        raise ValueError("Host Configuration internalParams must be a mapping")
    internal_params["selfUpgradeTrident"] = True
    config.pop("storage", None)
    return image["url"]


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Prepare an installed Host Configuration for a candidate A/B update."
    )
    parser.add_argument("source", type=Path)
    parser.add_argument("destination", type=Path)
    parser.add_argument("--version", type=int, required=True)
    args = parser.parse_args()

    config = yaml.safe_load(args.source.read_text(encoding="utf-8"))
    if not isinstance(config, dict):
        raise ValueError("Host Configuration root must be a mapping")

    update_url = prepare_host_config(config, args.version)
    args.destination.write_text(
        yaml.safe_dump(config, sort_keys=False),
        encoding="utf-8",
    )
    print(update_url)


if __name__ == "__main__":
    main()
