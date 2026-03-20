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


desc = """Return the Trident version.

When no BuildNumber is provided, the version from the cargo file will be
produced as-is, in the format MAJOR.MINOR.PATCH. 

If a BuildNumber is provided, the format will be MAJOR.MINOR.PATCH-YYYYMMDDID, where:
- MAJOR and MINOR are taken from the cargo file.
- YYYYMMDD is the date part of the BuildNumber.
- ID is the counter part of the BuildNumber, formatted as a two-digit number.

When the optional flag --commit is used, the short commit hash will be appended
to the version in the format MAJOR.MINOR.PATCH-YYYYMMDDID.vCOMMIT, where COMMIT is the
short commit hash.
"""

parser = argparse.ArgumentParser(description=desc)
parser.add_argument(
    "-c",
    "--commit",
    action="store_true",
    help="Optional flag to include prerelease version in output, where prerelease is YYYYMMDDID-vCOMMIT. See `BuildNumber` help for more details.",
)
parser.add_argument(
    "BuildNumber",
    type=str,
    help="Date and ID (counter) separated by a point. If not provided, the value from the cargo file will be produced.",
    nargs="?",
    default=None,
)

args = parser.parse_args()

with open("crates/trident/Cargo.toml", "r") as file:
    content = file.read()

# Format: MAJOR.MINOR.PATCH
version = get_version(content)

if args.BuildNumber is not None:
    version = get_version(content)

    # Check if BuildNumber is already the Trident version
    version_pattern = rf"({version})-(\d{{10}})(\.?.*)"
    if re.match(version_pattern, args.BuildNumber):
        print(args.BuildNumber)
    else:
        match = re.match(r"^(\d{8})\.(\d+)$", args.BuildNumber)
        if match is None:
            print(
                "Invalid input. BuildNumber should be a date and ID, for example a counter, separated by a point."
            )
            sys.exit(1)

        date, id = match.groups()
        id = int(id)

        if args.commit:
            short_commit = get_git_revision_short_hash()
            # Format: MAJOR.MINOR.PATCH-YYYYMMDDID.vCOMMIT
            print(f"{version}-{date}{id:02d}.v{short_commit.strip()}")
        else:
            print(f"{version}-{date}{id:02d}")
else:
    print(f"{version}")
