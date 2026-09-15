# Use a base-image catalogue

A **base-image catalogue** keeps base-image paths in one place and lets images reference them by name,
so the build always consumes a local file (no build-time registry pull). Use it when several images
share base files, when you build per-arch local bases, or when CI places base images out-of-band.

## 1. Declare the slots

Add a `baseImages:` list to `tailor.yaml`. Each slot has a unique `name`, a local `path`, and an optional remote `source`
that `tailor bases download` pulls from:

```yaml
# tailor.yaml
baseImages:
  - name: baremetal
    path: bases/baremetal.vhdx      # build input + download target (workspace-root-relative)
    arch: amd64
    source:
      azureLinux:
        version: "3.0"
        variant: baremetal
  - name: core_arm64
    path: bases/core_arm64.vhdx
    arch: arm64
    source:
      oci:
        uri: registry.example/core:3.0
  - name: qemu
    path: bases/qemu.vhdx           # no source: filled by a CI feed, download skips it
```

## 2. Reference a slot from an image

```yaml
# image.yaml — default base (amd64)
base:
  ref: baremetal
```

Swap slots per arch with a fragment — the slot's `arch` reconciles with the cell's:

```yaml
# by-arch/arm64.yaml
base:
  $set:
    ref: core_arm64
```

## 3. Fill the slots

Locally, download pulls every sourced slot whose file is missing (idempotent):

```console
$ tailor bases download          # all sourced, missing slots
$ tailor bases download baremetal --force   # re-pull one, even if present
```

In CI the feed step writes the same files; verify they landed before building:

```console
$ tailor bases verify            # fail fast if any referenced slot file is missing
```

## 4. See which slots the matrix needs

`tailor matrix --format json` tags each catalogue-backed cell with `baseImage`, so you download or
verify only what the selected cells use:

```console
$ tailor matrix --format json | jq -r '.[].baseImage' | sort -u
```

See [`baseImages` reference](../reference/tailor-yaml.md), [the `image` base kind](../reference/image-yaml.md),
and [Base images](../explanation/base-images.md).
