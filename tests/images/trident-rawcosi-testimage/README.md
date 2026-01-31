[![Build
Status](https://dev.azure.com/mariner-org/ECF/_apis/build/status%2FOneBranch%2Ftest-images%2Ftest-images-Official?repoName=test-images&branchName=main)](https://dev.azure.com/mariner-org/ECF/_build/latest?definitionId=2457&repoName=test-images&branchName=main)

# Trident Test Image for `trident stream-image`

This image is used for testing `trident stream-image` on Azure Linux. For AMD64, the image
is based on the baremetal image, and for ARM64, it is based on the ARM64 core
image. In both cases, the configuration adds Trident as well as its dependencies.
It also includes openssh-server to allow for remote access.

## Additional Prerequisites

- Artifacts
  - **Trident RPMs**: expected in `base/trident/*.rpm`. Can be downloaded with
    `make download-trident-rpms`

## Building

To build the base image and per-partition compressed images, run:

```bash
make trident-rawcosi-testimage
```
