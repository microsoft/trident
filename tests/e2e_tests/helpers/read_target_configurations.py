import argparse
from pathlib import Path
import sys
import yaml
import json
import logging

logging.basicConfig(level=logging.INFO)
log = logging.getLogger("read_target_configurations")


def main():
    parser = argparse.ArgumentParser(
        description="Reads a YAML file containing target configurations, "
        "selects the configurations based on the deployment environment, "
        "the build purpose, and the runtime environment of trident, and returns the "
        "configurations formatted into a matrix to define the pipeline jobs."
    )
    parser.add_argument(
        "-c",
        "--configurations",
        type=Path,
        required=True,
        help="File path to the YAML that contains the configurations for the E2E testing.",
    )
    parser.add_argument(
        "-e",
        "--env",
        type=str,
        required=True,
        choices=["virtualMachine", "bareMetal"],
        help="Deployment environment that will be used.",
    )
    parser.add_argument(
        "-p",
        "--purpose",
        type=str,
        required=True,
        help="The purpose of the build pipeline which influences the tests for E2E testing.",
    )
    parser.add_argument(
        "-r",
        "--runtimeEnv",
        type=str,
        required=True,
        choices=["host", "container"],
        help="The runtime environment of Trident (e.g., host or container).",
    )
    parser.add_argument(
        "--matrix-name",
        type=str,
        required=True,
        help="Name of the ADO variable to write the matrix to.",
    )
    args = parser.parse_args()

    log.info(
        f"Reading target configurations from '{args.configurations}' for '{args.env}' "
        f"with purpose '{args.purpose}' and runtime environment '{args.runtimeEnv}'."
    )

    configurations_file: Path = args.configurations.absolute()

    with open(configurations_file, "r") as file:
        target_configurations = yaml.safe_load(file)

    if args.env not in target_configurations:
        sys.exit(
            f"Deployment environment {args.env} not found in {args.configurations}."
        )
    elif args.runtimeEnv not in target_configurations[args.env]:
        sys.exit(
            f"Runtime environment {args.runtimeEnv} not found in {args.configurations} for {args.env}."
        )
    elif args.purpose not in target_configurations[args.env][args.runtimeEnv]:
        sys.exit(
            f"Build purpose {args.purpose} not found in {args.configurations} for {args.env} and {args.runtimeEnv}."
        )

    configurations = target_configurations[args.env][args.runtimeEnv][args.purpose]

    matrix = {name: {"configuration": name} for name in configurations}

    log.info(f"Matrix:\n{json.dumps(matrix, indent=4)}")

    print(
        f"##vso[task.setvariable variable={args.matrix_name};isOutput=true]{json.dumps(matrix)}"
    )


if __name__ == "__main__":
    main()
