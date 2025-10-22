# Builder

Builder is a Python tool to declare, customize, and build images with AZL Image Customizer locally and in the pipelines.

## Overview

Builder is a builder system designed around declarative image definitions that wraps around the AZL Image Customizer concepts and API.

`testimages.py` script in the top-level directory contains declarative definitions of all images and artifacts, such as the Image Customizer container. It is a convenient entry point for building an image locally or in the pipelines. To learn more about the supported commands, run:

```bash
python3 ./testimages.py --help
```

## Directory Structure

```md
builder/
├── __init__.py         # Definitions of wrappers around Image Customizer concepts
├── builder.py          # High-level logic for building image clones
├── cli.py              # Command-line interface and command execution
├── context_managers.py # Utilities for resource cleanup
├── customize.py        # Image Customizer API wrapper
├── download.py         # Utilities for image download
├── README.md           # README
├── run.py              # Core build functions and orchestration
└── sign.py             # Utilities for image signing
```

## Key Components

### `__init__.py`

Defines foundational data structures and enums used throughout Builder. Specifically, defines `ImageConfig`, which represents an Image Customizer config, and `ArtifactManifest`, which describes the Image Cuztomizer container to be used for building images.

Also, contains a series of other definitions that represent the base image type, output format, system architecture, etc.

### `cli.py`

Implements Builder's command-line interface and executes commands such as `build()`. Most of the high-level logic happens in `cli.init()`, which orchestrates the entire build process.

### `run.py`

Contains the implementations of the core functions supported by Builder, such as `build()` or `generate_matrix()`.

### `builder.py`

This is where the high-level logic around building images, signed and unsigned, lives. This file calls into Image Customizer APIs inside `customize.py` to build images, using cloning and parallel processing.

### `customize.py`

Wrapper around the AZL Image Customizer API. Only container-based execution is now supported since running IC as a raw binary is no longer supported.

Specifically, provides APIs for (1) building an image and (2) injecting signed boot artifacts into an image via the preview feature `inject-files`.

### Utility Files

#### `sign.py`

Utility functions needed for signing an image built via Image Customizer. This is needed for enabling `SecureBoot` in a host.

#### `download.py`

Utility functions for downloading images as AZ artifacts.

#### `context_managers.py`

Utility functions for resource cleanup.

## Key Concepts

### Image Cloning

The builder supports creating multiple clones of the same image with different UUIDs. This is essential for testing updates where you need identical images with unique identifiers:

### Parallel Processing

The system uses Python's `multiprocessing` package to build image clones in parallel, significantly reducing build times. Each clone is built in its own process with a deep copy of the `ImageConfig` object to avoid race conditions.

### Resource Management

The builder uses `ExitStack()` context managers to ensure proper cleanup of temporary resources.

## Running Builder

Builder can be run locally as a stand-alone tool:

```bash
    python3 ./testimages.py <command>
```

The following commands are supported:

```bash
    # List images available for building
    python3 ./testimages.py list
    # Download a base image: baremetal, core_selinux, qemu_guest, or minimal
    python3 ./testimages.py download-image <base_image_name>
    # List required dependencies for an image
    python3 ./testimages.py dependencies <image_name>
    # Build an image locally
    python3 ./testimages.py build <image_name>
    # Show key info about the image, such as name, source, config path, etc.
    python3 ./testimages.py show-image <image_name> <field>
    # Show info on key artifacts, such as the Image Customizer version or container image
    python3 ./testimages.py show-artifact
    # List images that have been built and are available to be used
    python3 ./testimages.py list-files
```

To build an image with Builder or test your changes to Builder, follow these steps:

1. If necessary, make changes to the Builder source code.
1. Download the base image:

```bash
    ./testimages.py download-image <base_image_name>
```

1. If necessary, update the Image Customizer config for the image you want to build, by modifying the corresponding YAML in `test-images/platform-integration-images`. You can also find the relevant YAML by running:

```bash
    python3 ./testimages.py show-image <image_name> config-file`

```

1. Build the image:

```bash
    python3 ./testimages.py build <image_name> --output-dir <output_dir> --no-download --clones <num_of_clones>
```
