
# Creating AzL Images to Deploy with Trident

## Goals

To deploy an operating system, Trident requires [COSI](../Reference/COSI.md) files. This document describes how to create a COSI file.

## Prerequisites

1. [Install Image Customizer](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start.html).

## Instructions

### Create OS Image

Follow the Image Customizer [documentation](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/quick-start/quick-start-binary.html) to configure and create an OS image, paying special attention to [specify](https://microsoft.github.io/azure-linux-image-tools/imagecustomizer/api/cli.html#--output-image-formatformat) `--output-image-format=cosi`.
