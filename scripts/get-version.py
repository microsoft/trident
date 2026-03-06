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


def get_version(file):
    pattern = r'version\s*=\s*"(\d+\.\d+\.\d+)"'

    match = re.search(pattern, file)

    if match:
        # Return the major.minor.patch version
        return match.group(1)
    else:
        print("Version definition not found.")
        sys.exit(1)


parser = argparse.ArgumentParser(
    description="Return the new version for Trident given the date and ID. Format: MAJOR.MINOR.PATCH"
)
parser.add_argument(
    "-c",
    "--commit",
    action="store_true",
    help="Optional flag to include prerelease version in output, where prerelease is YYYYMMDDID-vCOMMIT. See `BuildNumber` help for more details.",
)
parser.add_argument(
    "BuildNumber",
    type=str,
    help="BuildNumber is expected to either be in the format YYYYMMDD.ID (date.id) or MAJOR.MINOR.PATCH-YYYYMMDDID-vCOMMIT. If YYYYMMDD.ID is provided and `--commit` is specified, BuildNumber will be the source for prerelease version's YYYYMMDDID. If MAJOR.MINOR.PATCH-YYYYMMDDID-vCOMMIT is provided, it will be validated against the version in Cargo.toml and returned if valid.",
)

args = parser.parse_args()

with open("crates/trident/Cargo.toml", "r") as file:
    content = file.read()

# Format: MAJOR.MINOR.PATCH
version = get_version(content)

if not args.BuildNumber:
    print("Missing BuildNumber.")
    sys.exit(1)

match = re.match(r"(\d+)\.(\d+)", args.BuildNumber)

if match:
    # Check if BuildNumber is already the Trident version
    version_pattern = rf"^{version}-\d{{10}}(\..*)?$"

    if re.match(version_pattern, args.BuildNumber):
        print(args.BuildNumber)
    else:
        date, id = match.groups()
        id = int(id)

        if args.commit:
            short_commit = get_git_revision_short_hash()
            # Format: MAJOR.MINOR.PATCH-YYYYMMDDID.vCOMMIT
            print(f"{version}-{date}{id:02d}.v{short_commit.strip()}")
        else:
            print(f"{version}")
else:
    print(
        "Invalid input. BuildNumber should be a date and ID, for example a counter, separated by a point."
    )
    sys.exit(1)
