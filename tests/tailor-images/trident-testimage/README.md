# Trident Test Image

This image is used for testing Trident on Azure Linux. For AMD64, the image
is based on the baremetal image, and for ARM64, it is based on the ARM64 core
image. In both cases, the configuration adds Trident as well as its dependencies.
It also includes openssh-server to allow for remote access.

## Prerequisites

- **Trident RPMs** in `bin/RPMS/`. Build them with:
  ```bash
  make bin/trident-rpms.tar.gz
  ```

## Building

From the repo root, run:

```bash
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   build trident-testimage -s arch=amd64 --output-dir ./artifacts
```

Use `-s arch=arm64` for the ARM64 variant.

Output is written to `artifacts/trident-testimage.cosi` by default. Use
`--output-dir <path>` to change the output location.
