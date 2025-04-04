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
    pattern = r'version\s*=\s*"(\d+\.\d+)(\.\d+)"'

    match = re.search(pattern, file)

    if match:
        return match.group(1)
    else:
        print("Version definition not found.")
        sys.exit(1)


parser = argparse.ArgumentParser(
    description="Return the new version for Trident given the date and ID. Format: MAJOR.MINOR.YYYYMMDDID"
)
parser.add_argument(
    "-c",
    "--commit",
    action="store_true",
    help="Optional flag to use the short commit hash as part of the ID. Format: MAJOR.MINOR.YYYYMMDDID-COMMIT",
)
parser.add_argument(
    "BuildNumber", type=str, help="Date and ID (counter) separated by a point."
)

args = parser.parse_args()

with open("Cargo.toml", "r") as file:
    content = file.read()
version = get_version(content)


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
            print(f"{version}.{date}{id:02d}-v{short_commit.strip()}")
        else:
            print(f"{version}.{date}{id:02d}")
else:
    print(
        "Invalid input. BuildNumber should be a date and ID, for example a counter, separated by a point."
    )
