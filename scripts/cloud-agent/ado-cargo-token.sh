#!/usr/bin/env bash

set -euo pipefail

readonly ADO_RESOURCE_ID="499b84ac-1321-427f-aa17-267ca6975798"

printf 'Bearer '
az account get-access-token \
    --resource "$ADO_RESOURCE_ID" \
    --query accessToken \
    --output tsv
