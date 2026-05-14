# Trident Container Installer ISO Test Image

This image is used in the Trident test pipelines. At startup, it loads a
specified version of a container, which must be provided. Trident runs from
this container. The configuration for Trident can be patched into the ISO by
replacing it with the placeholder file (config-placeholder).

This image does **not** include Trident RPMs — Trident is loaded from a
container at boot.

## Building

From the repo root, run:

```bash
python3 tests/images/testimages.py build trident-container-installer
```

Output is written to `artifacts/trident-container-installer.iso` by default.
Use `--output-dir <path>` to change the output location.
