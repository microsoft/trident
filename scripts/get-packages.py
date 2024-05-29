#!/bin/python3

# # # # # # # # # # # # # # # # # # # # #
#             W A R N I N G             #
#   This script is used in pipelines!   #
#       Be careful when modifying!      #
# # # # # # # # # # # # # # # # # # # # #

import subprocess
import json
import logging
import argparse
from enum import Enum

logging.basicConfig(level=logging.INFO)


class Actions(Enum):
    """Actions that can be taken by the script."""

    INFO = "info"
    LATEST_VERSION = "latest"
    VERSION_EXISTS = "exists"


parser = argparse.ArgumentParser("Get package info from Devops!")

parser.add_argument("--org", default="https://dev.azure.com/mariner-org")
parser.add_argument("--project", default="ECF")
parser.add_argument("--feed", default="Trident")
parser.add_argument("-p", "--package")
parser.add_argument("-v", "--version")
parser.add_argument("--debug", action="store_true")
parser.add_argument(
    "--all-versions", action="store_true", help="Force fetching all versions"
)
parser.add_argument(
    "-a",
    "--action",
    choices=list(Actions),
    default=Actions.INFO,
    type=Actions,
    help="""Defines the action to be taken. 
                    'info' shows the information of the packages in the feed, 
                    'latest' returns the last version of the selected package, 
                    'exists' checks if the specified version is in the feed for the selected package.""",
)

args = parser.parse_args()

# Check for required arguments
if args.action == Actions.VERSION_EXISTS:
    if not args.version:
        logging.error("Missing argument: version")
        exit(1)
    if not args.package:
        logging.error("Missing argument: package")
        exit(1)

# Update logging level
logging.getLogger().setLevel(
    logging.DEBUG if parser.parse_args().debug else logging.INFO
)

# Base command
az_cmd = [
    "az",
    "devops",
    "invoke",
    "--org",
    args.org,
    "--api-version",
    "7.1",
    "--area",
    "packaging",
    "--resource",
    "packages",
    "--route-parameters",
    f"project={args.project}",
    f"feedId={args.feed}",
    "--http-method",
    "GET",
]

query_parameters = []

# When required, query only for a specific package name *pattern*. This is a
# pattern, not an exact match, so it does not guarantee a single package, but it
# does narrow down the search.
if args.package:
    logging.info(f"Retrieving querying specific pattern: {args.package}")
    query_parameters.append(f"packageNameQuery={args.package}")

# Only retrieve all versions when needed or when explicitly requested
if args.action == Actions.VERSION_EXISTS or args.all_versions:
    logging.debug("Retrieving all versions")
    query_parameters.append("includeAllVersions=True")

# If we have query parameters, append them to the command
if len(query_parameters) > 0:
    az_cmd.append("--query-parameters")
    az_cmd += query_parameters

logging.debug(f"Running command: {' '.join(az_cmd)}")
result = subprocess.run(
    az_cmd,
    check=True,
    text=True,
    capture_output=True,
)

# Parse the output
packages = json.loads(result.stdout)["value"]

# Extract version information
output = []
for package in packages:
    # Filter the output if a specific package was requested. The query filter is
    # just a pattern, not an exact match, so we need to filter manually to get
    # the exact package.
    if args.package and args.package != package["name"]:
        continue

    pkgout = {
        "name": package["name"],
        "id": package["id"],
    }

    versions = package["versions"]
    pkgout["versions"] = [version_info["version"] for version_info in versions]
    output.append(pkgout)

if not output:
    logging.error("No packages found!")
    exit(1)

if args.action == Actions.INFO:
    # Print the output
    print(json.dumps(output, indent=2))

elif args.action == Actions.LATEST_VERSION:
    # Get the latest version of each package
    latest_versions = {
        package["name"]: (
            package["versions"][0] if len(package["versions"]) > 0 else None
        )
        for package in output
    }

    # If a specific package was requested, print only that
    if args.package:
        latest = latest_versions.get(args.package)
        if latest:
            print(latest)
        else:
            logging.error(f"Package {args.package} has no known versions!")
            exit(1)

    # Otherwise, print all
    else:
        print(json.dumps(latest_versions, indent=2))

elif args.action == Actions.VERSION_EXISTS:
    # Check if the specified version is in the feed
    print(json.dumps(args.version in output[0]["versions"]))

else:
    logging.error("Invalid action")
    exit(1)
