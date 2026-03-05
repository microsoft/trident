#!/bin/python3

# # # # # # # # # # # # # # # # # # # # #
#             W A R N I N G             #
#   This script is used in pipelines!   #
#       Be careful when modifying!      #
# # # # # # # # # # # # # # # # # # # # #

import argparse
import re
import subprocess
import sys


def get_git_revision_short_hash() -> str:
    return (
        subprocess.check_output(["git", "rev-parse", "--short", "HEAD"])
        .decode("ascii")
        .strip()
    )


def get_version(file, use_date_as_patch):
    pattern = r'version\s*=\s*"(\d+\.\d+)(\.\d+)"'

    match = re.search(pattern, file)

    if match:
        if use_date_as_patch:
            # If using date as patch, just return major.minor
            return match.group(1)
        else:
            # if using patch from Cargo.toml, return major.minor.patch
            return f"{match.group(1)}{match.group(2)}"
    else:
        print("Version definition not found.")
        sys.exit(1)


parser = argparse.ArgumentParser(
    description="Return the new version for Trident given the date, ID, and commit. Format: MAJOR.MINOR.PATCH-YYYYMMDDID-vCOMMIT"
)
parser.add_argument(
    "-c",
    "--commit",
    action="store_true",
    help="Optional flag to use the short commit hash as part of the ID. Format: MAJOR.MINOR.PATCH-YYYYMMDDID-vCOMMIT",
)
parser.add_argument(
    "--build-type",
    type=str,
    choices=["local", "pipeline"],
    default="local",
    help="Defines what type of build to create, for local builds the date will be used as the patch version, for pipeline builds the patch will be read from Cargo.toml.",
)
parser.add_argument(
    "BuildNumber", type=str, help="Date and ID (counter) separated by a point."
)

args = parser.parse_args()

with open("crates/trident/Cargo.toml", "r") as file:
    content = file.read()

use_date_as_patch = args.build_type == "local"
version = get_version(content, use_date_as_patch)
next_separator = "." if use_date_as_patch else "-"

if not args.BuildNumber:
    print("Missing BuildNumber.")
    sys.exit()

match = re.match(r"(\d+)\.(\d+)", args.BuildNumber)

if match:
    # Check if BuildNumber is already the Trident version
    version_pattern = rf"(^{version}\.)(\d{{10}})(-?.*$)"
    if re.match(version_pattern, args.BuildNumber):
        print(args.BuildNumber)
    else:
        date, id = match.groups()
        id = int(id)

        if args.commit:
            short_commit = get_git_revision_short_hash()
            print(f"{version}{next_separator}{date}{id:02d}-v{short_commit.strip()}")
        else:
            print(f"{version}{next_separator}{date}{id:02d}")
else:
    print(
        "Invalid input. BuildNumber should be a date and ID, for example a counter, separated by a point."
    )
