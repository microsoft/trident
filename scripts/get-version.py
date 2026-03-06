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


def split_semver_version(file):
    pattern = r'version\s*=\s*"(\d+)\.(\d+)\.(\d+)"'

    match = re.search(pattern, file)

    if match:
        # Return the major, minor, and patch versions
        return match.group(1), match.group(2), match.group(3)
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
    "BuildNumber", type=str, help="Date and ID (counter) separated by a point."
)

args = parser.parse_args()

with open("crates/trident/Cargo.toml", "r") as file:
    content = file.read()

major, minor, patch = split_semver_version(content)

if not args.BuildNumber:
    print("Missing BuildNumber.")
    sys.exit(1)

match = re.match(r"(\d+)\.(\d+)", args.BuildNumber)

if match:
    # Check if BuildNumber is already the Trident version
    version_pattern = rf"^{major}\.{minor}\.{patch}-\d{{10}}(\..*)?$"

    if re.match(version_pattern, args.BuildNumber):
        print(args.BuildNumber)
    else:
        date, id = match.groups()
        id = int(id)

        basic_version = f"{major}.{minor}.{patch}"  # Format: MAJOR.MINOR.PATCH

        if args.commit:
            short_commit = get_git_revision_short_hash()
            # Format: MAJOR.MINOR.PATCH-YYYYMMDDID.vCOMMIT
            print(f"{basic_version}-{date}{id:02d}.v{short_commit.strip()}")
        else:
            print(f"{basic_version}")
else:
    print(
        "Invalid input. BuildNumber should be a date and ID, for example a counter, separated by a point."
    )
    sys.exit(1)
