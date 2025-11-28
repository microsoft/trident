BUILD_ID=$1

ARTIFACT_NAME=base-image-config
ARTIFACT_FILE=baseimage.json
ARTIFACT_DIR=/tmp/base-image-config

ADO_ORG_URL=${ADO_ORG_URL:-'https://dev.azure.com/mariner-org'}
ADO_PROJECT=${ADO_PROJECT:-'ECF'}

mkdir -p $ARTIFACT_DIR

if az pipelines runs artifact download \
        --org $ADO_ORG_URL --project $ADO_PROJECT \
        --artifact-name $ARTIFACT_NAME --path $ARTIFACT_DIR \
        --run-id $BUILD_ID; then
    echo "base image config artifact found, contents:"
    cat $ARTIFACT_DIR/$ARTIFACT_FILE

    BASEIMG_BUILD_TYPE=$(jq -r .baseimgBuildType $ARTIFACT_DIR/$ARTIFACT_FILE)
    BASE_IMAGE_PIPELINE_BUILD_ID=$(jq -r .baseImagePipelineBuildId $ARTIFACT_DIR/$ARTIFACT_FILE)
    BASE_IMAGE_ARM64_PIPELINE_BUILD_ID=$(jq -r .baseImageArm64PipelineBuildId $ARTIFACT_DIR/$ARTIFACT_FILE)
else
    echo "base image config artifact not found, using default settings (release, latest, latest)"

    BASEIMG_BUILD_TYPE="release"
    BASE_IMAGE_PIPELINE_BUILD_ID="latestFromBranch"
    BASE_IMAGE_ARM64_PIPELINE_BUILD_ID="latestFromBranch"
fi

echo "base image config:"
echo "BASEIMG_BUILD_TYPE = $BASEIMG_BUILD_TYPE"
echo "BASE_IMAGE_PIPELINE_BUILD_ID = $BASE_IMAGE_PIPELINE_BUILD_ID"
echo "BASE_IMAGE_ARM64_PIPELINE_BUILD_ID = $BASE_IMAGE_ARM64_PIPELINE_BUILD_ID"

echo "##vso[task.setvariable variable=BASEIMG_BUILD_TYPE]$BASEIMG_BUILD_TYPE"
echo "##vso[task.setvariable variable=BASE_IMAGE_PIPELINE_BUILD_ID]$BASE_IMAGE_PIPELINE_BUILD_ID"
echo "##vso[task.setvariable variable=BASE_IMAGE_ARM64_PIPELINE_BUILD_ID]$BASE_IMAGE_ARM64_PIPELINE_BUILD_ID"
