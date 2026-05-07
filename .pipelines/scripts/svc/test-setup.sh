#!/bin/bash
# test-setup.sh - Consolidated test job setup for servicing tests
# Combines artifact flattening, SSH setup, tool setup, and dependency install
# into a single script to reduce per-job YAML size.
#
# Required env vars:
#   ARTIFACTS - Build.ArtifactStagingDirectory path
#   TRIDENT_SOURCE_DIR - Trident source directory
#   OB_OUTPUT_DIR - Output directory for test artifacts
#   FLAVOR - Image flavor (qemu, azure, uki)
#   PLATFORM - Test platform (qemu, azure)
set -eux

# --- Flatten artifact subdirectories ---
# The batch build artifact has structure: {flavor}-base/, {flavor}-update-a/, {flavor}-update-b/
# Flatten to: $ARTIFACTS/ (base files), $ARTIFACTS/update-a/, $ARTIFACTS/update-b/

if [ -d "$ARTIFACTS/$FLAVOR-base" ]; then
    mv "$ARTIFACTS/$FLAVOR-base/"* "$ARTIFACTS/"
    rmdir "$ARTIFACTS/$FLAVOR-base"
fi

if [ -d "$ARTIFACTS/$FLAVOR-update-a" ]; then
    mkdir -p "$ARTIFACTS/update-a"
    mv "$ARTIFACTS/$FLAVOR-update-a/"* "$ARTIFACTS/update-a/"
    rmdir "$ARTIFACTS/$FLAVOR-update-a"
fi

if [ -d "$ARTIFACTS/$FLAVOR-update-b" ]; then
    mkdir -p "$ARTIFACTS/update-b"
    mv "$ARTIFACTS/$FLAVOR-update-b/"* "$ARTIFACTS/update-b/"
    rmdir "$ARTIFACTS/$FLAVOR-update-b"
fi

# --- Set up SSH keys ---
cp "$ARTIFACTS/ssh/id_rsa"* ~/.ssh/
chmod -R 700 ~/.ssh/

# --- Set up go tools ---
chmod +x "$TRIDENT_SOURCE_DIR/bin/netlisten"
chmod +x "$TRIDENT_SOURCE_DIR/bin/storm-trident"

# --- Create output directory ---
mkdir -p "$OB_OUTPUT_DIR"

# --- Install platform-specific dependencies ---
if [ "$PLATFORM" == "qemu" ]; then
    sudo apt-get update -qq
    sudo apt-get install -y -qq imagemagick
fi

# --- Initialize test state variable ---
echo "##vso[task.setvariable variable=STORM_SCENARIO_FINISHED;]false"
