# How-to guides

How-to guides are task-oriented recipes. Most assume a **workspace** (`tailor.yaml` plus one or
more `image.yaml` files); actual `build` and `convert` runs need a container **engine**, while
`render`, `validate`, and `build --dry-run` do not. Some steps need a **registry** login or a
base-image **catalogue**. Each guide notes anything extra it needs.

## Author a workspace

- [Add an image](add-an-image.md)
- [Add an axis](add-an-axis.md)
- [Share config with `$include`](share-config-with-include.md)
- [Override a base per axis](override-a-base-per-axis.md)
- [Build dependent images](build-dependent-images.md)
- [Track local build inputs](track-build-inputs.md)

## Choose a base image

- [Build an image from an Azure Linux base](build-with-an-azure-linux-base.md)
- [Use a base-image catalogue](use-a-base-image-catalogue.md)
- [Use a tools dir for sealed images](use-a-tools-dir.md)

## Build and select

- [Select and build one cell](select-and-build-one-cell.md)
- [Cross-arch building](cross-arch-building.md)
- [Build clones of a cell](build-clones.md)
- [Change an output format](change-an-output-format.md)
- [Convert an image format](convert-an-image-format.md)
- [Set the build directory](set-the-build-directory.md)
- [Clean and rebuild a cell](clean-and-rebuild.md)
- [Manage extracted Image Customizer artifacts](manage-extracted-artifacts.md)
- [Skip experimental cells in bulk runs](skip-experimental-cells.md)

## Toolchains

- [Select an Image Customizer tag](pin-the-ic-version.md)
- [Use a locally-built Image Customizer image](use-a-local-ic-image.md)

## Environment and engine

- [Select a container engine (Docker or Podman)](select-a-container-engine.md)
- [Capture Image Customizer logs](capture-build-logs.md)

## Pipelines and CI

- [Export configs for a pipeline](export-configs-for-a-pipeline.md)
- [Generate a CI matrix](generate-a-ci-matrix.md)
- [Handle exit codes in scripts](handle-exit-codes.md)

## Signing and preview features

- [Enable a preview feature](use-preview-features.md)
- [Sign an image](sign-an-image.md)

## Distribute and comply

- [Build a portable binary](build-a-portable-binary.md)
- [Print third-party license notices](print-license-notices.md)
