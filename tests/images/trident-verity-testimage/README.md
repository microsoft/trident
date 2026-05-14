# Trident Verity Test Image

This image is used for testing Trident on Azure Linux with `dm-verity` enabled. It
is based on the baremetal image and adds Trident as well as its dependencies. It
also adds openssh-server to allow for remote access. In addition to the [Trident
Test Image](../trident-testimage/README.md), this image is configured to use
dm-verity.

## Prerequisites

- **Trident RPMs** in `bin/RPMS/`. Build them with:
  ```bash
  make bin/trident-rpms.tar.gz
  ```

## Building

From the repo root, run:

```bash
python3 tests/images/testimages.py build trident-verity-testimage
```

Output is written to `artifacts/trident-verity-testimage.cosi` by default. Use
`--output-dir <path>` to change the output location.
