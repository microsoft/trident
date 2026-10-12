# Test images with tailor

`tests/tailor-images/` is a [tailor](../../tools/tailor/README.md) workspace.
It replaces the legacy `testimages.py` / `builder/` flow, which has been removed.

## Build from the repo root

Use the in-tree tailor binary via Cargo:

```bash
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   matrix trident-installer --format json
```

Build examples:

```bash
# trident-testimage
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   build trident-testimage -s arch=amd64 --output-dir ./artifacts

# trident-testimage-arm64
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   build trident-testimage -s arch=arm64 --output-dir ./artifacts

# trident-direct-streaming-installer-arm64
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   build trident-installer -s arch=arm64 -s variant=direct-streaming --output-dir ./artifacts

# trident-container-usrverity-testimage (signed)
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   build trident-verity-testimage -s deployment=container -s mode=usr --output-dir ./artifacts
```

## Base images

The workspace uses a `baseImages:` catalogue in `tests/tailor-images/tailor.yaml`.
For the Azure Linux base slots you can materialize the defaults locally with:

```bash
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   bases download baremetal core_arm64 core_selinux qemu_guest
```

Verify that every referenced base file exists:

```bash
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   bases verify
```

Ubuntu / GB200 direct-streaming inputs still come from the pipeline-managed blob
staging flow; those slots are intentionally catalogued without an in-workspace
remote source.

## Image families

The current families are:

- `azl-installer`
- `azurelinux-direct-streaming-testimage`
- `foreign-direct-streaming-testimage`
- `trident-container-installer`
- `trident-container-testimage`
- `trident-functest`
- `trident-installer`
- `trident-testimage`
- `trident-verity-testimage`
- `trident-vm-testimage`
