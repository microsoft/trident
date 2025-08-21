#!/usr/bin/env bash

set -euo pipefail

# Script to create versioned documentation using GitHub releases
# This script fetches all releases from the repository and creates versioned docs

WEBSITE_SCRIPTS_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )"

# Configuration
REPO="microsoft/trident"
DOCS_NAME=docs
WEBSITE_DIR="$WEBSITE_SCRIPTS_DIR/.."
VERSIONED_DOCS_NAME=versioned_docs
VERSIONED_DOCS_DIR="$WEBSITE_SCRIPTS_DIR/../$VERSIONED_DOCS_NAME"
VERSIONED_SIDEBARS_NAME=versioned_sidebars
VERSIONED_SIDEBARS_DIR="$WEBSITE_SCRIPTS_DIR/../$VERSIONED_SIDEBARS_NAME"
VERSIONS_FILE="$WEBSITE_SCRIPTS_DIR/../versions.json"

# DEV_BRANCH="main"
DEV_BRANCH="user/bfjelds/docusaurus-poc"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if gh CLI is installed
check_gh_cli() {
    if ! command -v gh &> /dev/null; then
        log_error "GitHub CLI (gh) is not installed. Please install it first."
        exit 1
    fi
    
    # Check if authenticated
    if ! gh auth status &> /dev/null; then
        log_error "GitHub CLI is not authenticated. Please run 'gh auth login' first."
        exit 1
    fi
    
    log_success "GitHub CLI is installed and authenticated"
}

# Get all releases using GitHub CLI
get_releases() {
    # Get all releases (excluding pre-releases by default)
    releases=$(gh api "repos/${REPO}/releases" --jq ".[] | select(.prerelease=false) | .name" --paginate)
    if [[ "${1:-}" != "--include-prerelease" ]]; then
        releases=$(gh api "repos/${REPO}/releases" --jq ".[] | .name" --paginate)
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

# Create version directory structure
create_version_docs() {
    local version="$1"
    local tmp_dir=$(mktemp -d)
    cd "${tmp_dir}"
    
    log_info "Checkout ${version} in ${tmp_dir}"
    git clone --depth 1 --branch "${version}" "https://github.com/${REPO}.git" "${tmp_dir}"
    cd "${tmp_dir}"/website
    npm install

    log_info "Creating documentation for version ${version}"

    if [[ "$version" != "$DEV_BRANCH" ]]; then
        local normalized_version=$(echo "$version" | sed 's|/|-|')
        log_info "Move version docs folder to website/docs"
        log_info "Use docusaurus versioning to create versioned_*"
        npm run docusaurus docs:version $normalized_version
        log_info "Copy versioned_* to website"
        cp -r "${tmp_dir}/website/$VERSIONED_DOCS_NAME/version-$normalized_version" "${VERSIONED_DOCS_DIR}/"
        cp -r "${tmp_dir}/website/$VERSIONED_SIDEBARS_NAME/version-$normalized_version-sidebars.json" "${VERSIONED_SIDEBARS_DIR}/"
    else
        log_info "For dev branch, copy docs to website"
        # For dev branch, copy docs to website/docs
        cp -r "${tmp_dir}/docs" "$WEBSITE_DIR/"
    fi

    rm -rf ${tmp_dir}
}

# Update versions.json file
update_versions_file() {
    local versions=("$1")
    
    log_info "Updating ${VERSIONS_FILE}"
    
    # Create versions.json with all version tags
    echo "$(echo "$versions" | jq -R -s 'split("\n")[:-1]')" > "$VERSIONS_FILE"
    cat $VERSIONS_FILE
    
    log_success "Updated ${VERSIONS_FILE}"
}

# Main function
main() {
    local include_prerelease=""
    local force_recreate=""
    
    # Parse command line arguments
    while [[ $# -gt 0 ]]; do
        case $1 in
            --include-prerelease)
                include_prerelease="--include-prerelease"
                shift
                ;;
            --force)
                force_recreate="--force"
                shift
                ;;
            --help)
                echo "Usage: $0 [OPTIONS]"
                echo ""
                echo "Options:"
                echo "  --include-prerelease    Include pre-release versions"
                echo "  --force                 Force recreate existing version directories"
                echo "  --help                  Show this help message"
                exit 0
                ;;
            *)
                log_error "Unknown option: $1"
                exit 1
                ;;
        esac
    done
    
    log_info "Starting versioned docs creation..."
    
    # Check prerequisites
    check_gh_cli

    #
    # Can use branches for testing
    #
    # versions=$(get_branches "releases/")
    # log_info "Found branches: ${versions[*]}"

    # Get releases
    versions=$(get_releases $include_prerelease)
    log_info "Found releases: ${versions[*]}"
    
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
    
    log_success "Versioned documentation creation completed!"
    log_info "Created versions: $(echo $versions)"
}

# Run main function with all arguments
main "$@"