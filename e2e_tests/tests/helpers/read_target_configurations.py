import argparse
import sys
import yaml


def format_matrix(configurations):
    return {directory: {"configuration": directory} for directory in configurations}


def main():
    parser = argparse.ArgumentParser(
        description="Reads a YAML file containing target configurations, "
        "selects the configurations based on the deployment environment and "
        "the build purpose of the pipeline, and returns "
        "the configuration formatted into a matrix for the pipeline to define jobs."
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
        help="Deployment environment that will be used.",
    )
    parser.add_argument(
        "-p",
        "--purpose",
        type=str,
        required=True,
        help="The purpose of the build pipeline which influences the tests for E2E testing.",
    )
    args = parser.parse_args()

    with open(args.configurations, "r") as file:
        target_configurations = yaml.safe_load(file)

    if not args.env in target_configurations:
        sys.exit(
            f"Deployment environment {args.env} not found in {args.configurations}."
        )
    elif not args.purpose in target_configurations[args.env]:
        sys.exit(
            f"Build purpose {args.purpose} not found in {args.configurations} for {args.env}."
        )
    else:
        matrix = format_matrix(target_configurations[args.env][args.purpose])
        print(matrix)


if __name__ == "__main__":
    main()
