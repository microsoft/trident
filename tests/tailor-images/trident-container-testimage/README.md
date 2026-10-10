# Trident Container Test Image

This image is an extension of the base baremetal image and includes various
helper utilities along with Docker runtime. It also adds openssh-server to
allow for remote access.

This image does **not** include Trident RPMs — Trident runs from a container
loaded at runtime.

## Building

From the repo root, run:

```bash
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/images/tailor.yaml   build trident-container-testimage --output-dir ./artifacts
```

Output is written to `artifacts/trident-container-testimage.cosi` by default.
Use `--output-dir <path>` to change the output location.
