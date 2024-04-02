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

parser.add_argument(
    "-f", "--filter", default=None, help="Match a specific package name"
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
    if args.filter and args.filter != package["name"]:
        continue
    pkgout = {
        "name": package["name"],
        "id": package["id"],
    }

    versions = package["versions"]
    pkgout["latestVersion"] = versions[0]["version"] if len(versions) > 0 else None
    output.append(pkgout)

print(json.dumps(output, indent=2))
