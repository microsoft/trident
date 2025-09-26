#!/usr/bin/env bash

set -euo pipefail

# Script to create versioned documentation using GitHub releases
# This script fetches all releases from the repository and creates versioned docs

WEBSITE_SCRIPTS_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"

DEBUG_USE_DEV_BRANCH=${DEBUG_USE_DEV_BRANCH:-false}
DEBUG_USE_BRANCHES=${DEBUG_USE_BRANCHES:-false}
DEBUG_BRANCH_PATTERN=${DEBUG_BRANCH_PATTERN:-'releases/'}
MAX_VERSION_COUNT=${MAX_VERSION_COUNT:-'-1'}

EXCLUDED_VERSIONS=${EXCLUDED_VERSIONS:-''}
DEV_BRANCH=${DEV_BRANCH:-'main"}

# Configuration
REPO="microsoft/trident"
DOCS_NAME=docs
WEBSITE_DIR="$WEBSITE_SCRIPTS_DIR/.."
VERSIONED_DOCS_NAME=versioned_docs
VERSIONED_DOCS_DIR="$WEBSITE_SCRIPTS_DIR/../$VERSIONED_DOCS_NAME"
VERSIONED_SIDEBARS_NAME=versioned_sidebars
VERSIONED_SIDEBARS_DIR="$WEBSITE_SCRIPTS_DIR/../$VERSIONED_SIDEBARS_NAME"
VERSIONS_FILE="$WEBSITE_SCRIPTS_DIR/../versions.json"


# Check if gh CLI is installed
check_gh_cli() {
    if ! command -v gh &> /dev/null; then
        echo "GitHub CLI (gh) is not installed. Please install it first."
        exit 1
    fi
    
    # Check if authenticated
    if ! gh auth status &> /dev/null; then
        echo "GitHub CLI is not authenticated. Please run 'gh auth login' first."
        exit 1
    fi
    
    echo "GitHub CLI is installed and authenticated"
}

# Get all releases using GitHub CLI
get_releases() {
    local include_prerelease="$1"
    if [[ "$include_prerelease" == "true" ]]; then
        # Get all releases (including prereleases)
        releases=$(gh api "repos/${REPO}/releases" --jq ".[] | .name" --paginate)
    else
        # Get only non-prerelease releases (exclude prereleases)
        releases=$(gh api "repos/${REPO}/releases" --jq ".[] | select(.prerelease==false) | .name" --paginate)
    fi
    echo "$releases"
}

# Get all branches using GitHub CLI
get_branches() {
    local pattern="$1"
    # Get all branches using the GitHub API
    branches=$(gh api "repos/${REPO}/branches" --jq ".[] | select(.name | contains(\"${pattern}\")) | .name" --paginate | tac)
    echo "$branches"
}

# Avoid specified excluded versions
exclude_versions() {
    local all_versions="$1"

    if [[ "$EXCLUDED_VERSIONS" == "" ]]; then
        echo "$all_versions"
    else
        while read -r unfiltered_version; do
            found=false
            while read -r excluded_version; do
                if [[ "$unfiltered_version" == "$excluded_version" ]]; then
                    found=true
                    break
                fi
            done <<< "$(echo "${EXCLUDED_VERSIONS}" | tr ' ' '\n')"
            if ! $found; then
                echo "$unfiltered_version"
            fi
        done <<< "$all_versions"
    fi
}

# Create version directory structure
create_version_docs() {
    local version="$1"
    local tmp_dir=$(mktemp -d)
    cd "${tmp_dir}"
    
    echo "Checkout ${version} in ${tmp_dir}"
    if [[ "$DEBUG_USE_DEV_BRANCH" == "true" ]]; then
        # Debug: clone the dev branch
        gh repo clone "https://github.com/${REPO}.git" "${tmp_dir}" -- --depth 1 --branch "${DEV_BRANCH}"
    else
        gh repo clone "https://github.com/${REPO}.git" "${tmp_dir}" -- --depth 1 --branch "${version}" 
    fi
    cd "${tmp_dir}"/website
    npm install

    echo "Creating documentation for version ${version}"

    if [[ "$version" != "$DEV_BRANCH" ]]; then
        local normalized_version=$(echo "$version" | sed 's|/|-|')
        echo "Move version docs folder to website/docs"
        echo "Use docusaurus versioning to create versioned_*"
        npm run docusaurus docs:version "$normalized_version"
        echo "Copy versioned_* to website"
        cp -r "${tmp_dir}/website/$VERSIONED_DOCS_NAME/version-$normalized_version" "${VERSIONED_DOCS_DIR}/"
        cp -r "${tmp_dir}/website/$VERSIONED_SIDEBARS_NAME/version-$normalized_version-sidebars.json" "${VERSIONED_SIDEBARS_DIR}/"
    else
        echo "For dev branch, copy docs to website"
        # For dev branch, copy docs to website/docs
        cp -r "${tmp_dir}/docs" "$WEBSITE_DIR/"
    fi

    rm -rf "${tmp_dir}"
}

# Update versions.json file
update_versions_file() {
    local versions=("$1")
    
    echo "Updating ${VERSIONS_FILE}"
    
    # Create versions.json with all version tags
    echo "$(echo "$versions" | jq -R -s 'split("\n")[:-1]')" > "$VERSIONS_FILE"
    cat $VERSIONS_FILE
    
    echo "Updated ${VERSIONS_FILE}"
}

# Main function
main() {
    local include_prerelease="false"
    
    # Parse command line arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            --include-prerelease)
                include_prerelease="true"
                shift
                ;;
            --help)
                echo "Usage: $0 [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  --include-prerelease    Include pre-release versions"
                echo "  --help                  Show this help message"
                exit 0
                ;;
            *)
                echo "Unknown option: $1"
                exit 1
                ;;
        esac
    done
    
    echo "Starting versioned docs creation..."
    
    # Check prerequisites
    check_gh_cli

    if [[ "$DEBUG_USE_BRANCHES" == "true" ]]; then
        #
        # Can use branches for testing
        #
        versions=$(get_branches "$DEBUG_BRANCH_PATTERN")
        echo "Found branches: ${versions}"
    else
        # Get releases
        versions=$(get_releases $include_prerelease)
        echo "Found releases: ${versions}"
    fi

    versions=$(exclude_versions "$versions")
    echo "Filtered versions: ${versions}"
    
    if [[ "$MAX_VERSION_COUNT" != "-1" ]]; then
        versions=$(echo "$versions" | head -n "$MAX_VERSION_COUNT")
        echo "Count-limited versions: ${versions}"
    fi

    # Create versioned docs directory if it doesn't exist
    mkdir -p "$VERSIONED_DOCS_DIR"
    mkdir -p "$VERSIONED_SIDEBARS_DIR"
    
    # Process each version
    if [[ "$(echo "$versions" | xargs)" != "" ]]; then
        while read -r version; do
            if [[ "$(echo "$version" | xargs)" != "" ]]; then
                echo "Processing version: ${version}"
                create_version_docs "$version"
            fi
        done <<< "$versions"
    fi
    # Create dev-branch version docs
    create_version_docs "$DEV_BRANCH"
    if [[ "$(echo "$versions" | xargs)" != "" ]]; then
        # Add dev-branch to versions
        versions=$(echo -e "$versions\ncurrent")
    else
        # Add dev-branch to versions
        versions="current"
    fi

    # Update versions.json
    update_versions_file "$(echo -e "$versions" | sed 's|/|-|')"
    
    echo "Versioned documentation creation completed!"
    echo "Created versions: $(echo $versions)"
}

# Run main function with all arguments
main "$@"