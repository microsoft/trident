# Trident Installer ISO Test Image

This image is used by the Trident test pipelines. It contains a copy of Trident that launches at
startup and reads a configuration that can be patched into the ISO.

## Building

From the repo root, run:

```bash
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   build trident-installer -s arch=amd64 -s variant=default --output-dir ./artifacts
```

Other legacy variants map to selectors on the same family:

- `trident-split-installer` → `-s arch=amd64 -s variant=split`
- `trident-direct-streaming-installer-amd64` → `-s arch=amd64 -s variant=direct-streaming`
- `trident-installer-arm64` → `-s arch=arm64 -s variant=default`
- `trident-direct-streaming-installer-arm64` → `-s arch=arm64 -s variant=direct-streaming`
