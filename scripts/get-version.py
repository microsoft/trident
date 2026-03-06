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


def get_versions(file):
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

if not args.BuildNumber:
    print("Missing BuildNumber.")
    sys.exit(1)

match = re.match(r"(\d+)\.(\d+)", args.BuildNumber)
if not match:
    print(
        "Invalid input. BuildNumber should be a date and ID, for example a counter, separated by a point."
    )
    sys.exit(1)

with open("crates/trident/Cargo.toml", "r") as file:
    content = file.read()

major, minor, patch = get_versions(content)

date_pattern = rf"\d{{10}}"
basic_version_pattern = rf"{major}\.{minor}\.{patch}"  # major.minor.patch
prerelease_pattern = rf".*"
version_pattern = rf"^{basic_version_pattern}-?{prerelease_pattern}$"  # major.minor.date-rest or major.minor.patch-rest

if re.match(version_pattern, args.BuildNumber):
    print(args.BuildNumber)
else:
    date, id = match.groups()
    id = int(id)

    basic_version = f"{major}.{minor}.{patch}"  # Format: MAJOR.MINOR.PATCH

    if args.commit:
        short_commit = f"v{get_git_revision_short_hash().strip()}"
        # Format: MAJOR.MINOR.PATCH-YYYYMMDDID.vCOMMIT
        print(f"{basic_version}-{date}{id:02d}.{short_commit}")
    else:
        print(f"{basic_version}")
