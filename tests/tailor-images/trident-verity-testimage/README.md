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
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/images/tailor.yaml   build trident-verity-testimage -s deployment=host -s mode=root --output-dir ./artifacts
```

Other legacy variants map to selectors on the same family:

- `trident-usrverity-testimage` → `-s deployment=host -s mode=usr`
- `trident-container-verity-testimage` → `-s deployment=container -s mode=root`
- `trident-container-usrverity-testimage` → `-s deployment=container -s mode=usr`

Output is written to `artifacts/` using the tailor cell slug by default. Use
`--output-dir <path>` to change the output location.
