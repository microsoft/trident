#!/bin/python3

import argparse
import os
import yaml
import json


def check_trident_config_fields(trident_config_file):
    with open(trident_config_file, "r") as config:
        trident_config_data = yaml.safe_load(config)

    # Check for specific fields in the trident config
    fields_status = {
        "raid_enabled": "raid" in trident_config_data["hostConfiguration"]["storage"],
        "encryption_enabled": "encryption"
        in trident_config_data["hostConfiguration"]["storage"],
        "abUpdate_enabled": "abUpdate"
        in trident_config_data["hostConfiguration"]["storage"],
        "verity_enabled": "verity"
        in trident_config_data["hostConfiguration"]["storage"],
    }
    return fields_status


def process_metrics(metrics_file, fields_status):
    # Read the metrics file and update the additional fields based on which
    # trident config fields are enabled
    # Also add the pipeline build id and trident commit hash to the metrics
    updated_metrics = []
    with open(metrics_file, "r") as file:
        for line in file:
            metric = json.loads(line)
            metric["platform_info"]["pipeline_name"] = os.environ.get(
                "PIPELINE_NAME", "Unknown"
            )
            metric["platform_info"]["pipeline_build_id"] = os.environ.get(
                "BUILD_BUILDID", "Unknown"
            )
            metric["platform_info"]["pipeline_agent_sku"] = os.environ.get(
                "PIPELINE_AGENT_SKU", "Unknown"
            )
            metric["platform_info"]["environment"] = os.environ.get(
                "TEST_ENVIRONMENT", "Unknown"
            )
            metric["platform_info"]["location"] = os.environ.get(
                "TEST_LOCATION", "Unknown"
            )
            metric["platform_info"]["server_name"] = os.environ.get(
                "TEST_SERVER_NAME", "Unknown"
            )
            metric["platform_info"]["branch"] = os.environ.get(
                "SOURCE_BRANCH_NAME", "Unknown"
            )
            metric["platform_info"]["machine_type"] = os.environ.get(
                "MACHINE_TYPE", "Unknown"
            )
            metric["additional_fields"]["trident_config_name"] = os.environ.get(
                "TRIDENT_CONFIGURATION_NAME", "Unknown"
            )
            metric["additional_fields"]["trident_commit_hash"] = os.environ.get(
                "BUILD_SOURCEVERSION", "Unknown"
            )

            # Update the additional_fields based on the trident config
            for key, enabled in fields_status.items():
                if enabled:
                    metric["additional_fields"][key] = True
            updated_metrics.append(json.dumps(metric))

    # Write the updated metrics back to the file
    with open(metrics_file, "w") as file:
        for metric in updated_metrics:
            file.write(metric + "\n")


def main():
    print("Processing Trident metrics...")
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--trident-config",
        "-t",
        help="Path to the Trident configuration file",
        required=True,
        type=str,
    )
    parser.add_argument(
        "--metrics-file",
        "-m",
        help="Path to the metrics file",
        required=True,
        type=str,
    )
    args = parser.parse_args()

    fields_status = check_trident_config_fields(args.trident_config)
    process_metrics(args.metrics_file, fields_status)

    print("Trident metrics processed successfully!")


if __name__ == "__main__":
    main()
