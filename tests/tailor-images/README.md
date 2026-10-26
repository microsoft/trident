# Test images with tailor

`tests/tailor-images/` is a [tailor](../../tools/tailor/README.md) workspace.
It replaces the legacy `testimages.py` / `builder/` flow, which runs alongside it
for now and is removed in a follow-up cleanup PR.

## Build from the repo root

Use the in-tree tailor binary via Cargo:

```bash
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   matrix trident-testimage --format json
```

Build examples:

```bash
# trident-testimage
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   build trident-testimage -s arch=amd64 --output-dir ./artifacts

# trident-testimage-arm64
cargo run --manifest-path tools/tailor/Cargo.toml --   --manifest tests/tailor-images/tailor.yaml   build trident-testimage -s arch=arm64 --output-dir ./artifacts
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

- `trident-functest`
- `trident-testimage`
