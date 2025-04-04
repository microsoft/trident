import argparse
import sys
import yaml


def format_matrix(configurations):
    return {directory: {"configuration": directory} for directory in configurations}


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
        type=str,
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
    args = parser.parse_args()

    with open(args.configurations, "r") as file:
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
    else:
        matrix = format_matrix(
            target_configurations[args.env][args.runtimeEnv][args.purpose]
        )
        print(matrix)


if __name__ == "__main__":
    main()
