#!/bin/bash

option="$1"

case $option in
    pr)
        >&2 echo "You chose pr"
        PIPELINE_ID=2113
        # Add your pr related code here
        ;;
    ci)
        >&2 echo "You chose ci"
        PIPELINE_ID=2195
        ;;
    pre)
        >&2 echo "You chose pre"
        PIPELINE_ID=2648
        # Add your pre related code here
        ;;
    rel)
        >&2 echo "You chose rel"
        PIPELINE_ID=0
        # Add your rel related code here
        ;;
    *)
        >&2 echo "Invalid option. Please choose from pr, ci, pre, rel."
        exit 1
        ;;
esac

BRANCH=$(git rev-parse --abbrev-ref HEAD)

TEMPDIR=$(mktemp -d)

cat > $TEMPDIR/payload.json << EOF
{
    "previewRun": true,
    "resources": {
        "repositories": {
            "self": {
                "refName": "refs/heads/$BRANCH"
            }
        }
    }
}
EOF

RESPONSE=$(az devops invoke \
    --org https://dev.azure.com/mariner-org \
    --api-version 7.0 \
    --area pipelines \
    --resource runs \
    --route-parameters project="ECF" pipelineId=$PIPELINE_ID \
    --http-method POST \
    --in-file $TEMPDIR/payload.json)

echo $RESPONSE | jq -r '.finalYaml'