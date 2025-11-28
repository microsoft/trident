#!/bin/bash

set -e

# Parameters passed to the script
METRICS_FILE="$1"
KUSTO_DATABASE_NAME="${2:-trident}"
KUSTO_TABLE_NAME="$3"
KUSTO_TABLE_MAPPING="${4:-metrics}"  # Default to "metrics"
PLATFORM_TELEMETRY_REPO_PATH="$5"

# Set up Python environment and install required packages
echo "Setting up Python virtual environment and installing dependencies..."
cd "$PLATFORM_TELEMETRY_REPO_PATH"
python3 -m venv venv
source venv/bin/activate
pip3 install -r pipelines/templates/requirements.txt

# Upload metrics to Kusto
echo "Uploading metrics to Kusto..."

python bmp-kusto-scripts/kusto_ingestor.py \
    --database "$KUSTO_DATABASE_NAME" \
    --table "$KUSTO_TABLE_NAME" \
    --filepath "$METRICS_FILE" \
    --mapping "$KUSTO_TABLE_MAPPING"

echo "Metrics uploaded successfully to Kusto."
