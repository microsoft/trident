#!/bin/python3

# # # # # # # # # # # # # # # # # # # # #
#             W A R N I N G             #
#   This script is used in pipelines!   #
#       Be careful when modifying!      #
# # # # # # # # # # # # # # # # # # # # #

import subprocess
import json
import argparse

parser = argparse.ArgumentParser("Get package info from Devops!")

parser.add_argument("--org", default="https://dev.azure.com/mariner-org")
parser.add_argument("--project", default="ECF")
parser.add_argument("--feed", default="Trident")
parser.add_argument("--package", default="")
parser.add_argument("--version", default="")

parser.add_argument(
    "--action",
    choices=["info", "latestVersion", "isVersionInFeed"],
    default="info",
    help="""Defines the action to be taken. 
                    'info' shows the information of the packages in the feed, 
                    'latestVersion' returns the last version of the selected package, 
                    'isVersionInFeed' checks if the specified version is in the feed for the selected package.""",
)


args = parser.parse_args()

result = subprocess.run(
    [
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
        "--query-parameters",
        "includeAllVersions=True",
        f"packageNameQuery={args.package}",
        "--route-parameters",
        f"project={args.project}",
        f"feedId={args.feed}",
        "--http-method",
        "GET",
    ],
    check=True,
    text=True,
    capture_output=True,
)

packages = json.loads(result.stdout)["value"]

output = []
for package in packages:
    pkgout = {
        "name": package["name"],
        "id": package["id"],
    }

    versions = package["versions"]
    pkgout["versions"] = [version_info["version"] for version_info in versions]
    output.append(pkgout)

if not output:
    print("No matching packages found.")
elif args.action == "info":
    print(json.dumps(output, indent=2))
elif not args.package:
    print("Missing argument: package")
elif args.action == "latestVersion":
    print(
        output[0]["versions"][0]
        if len(output[0]["versions"]) > 0
        else f"No versions of {args.package}"
    )
elif not args.version:
    print("Missing argument: version")
else:
    print(args.version in output[0]["versions"])
