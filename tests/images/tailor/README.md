# Tailor test images

This directory contains declarative image models built by Tailor using Image
Customizer. The existing Make/Python image definitions outside this directory
remain available; a new model does not move its consumers automatically.

## Layout

```text
tailor.yaml
common/
  files/                 Shared files copied into images
  scripts/azl4/          Shared inline Bash snippets in YAML
trident-vm-testimage/
  image.yaml             Inputs, matrix axes, parameters and output defaults
  by-scenario/           Base QCOW2 versus update COSI configuration
  by-distro/             Distro-specific overrides
  by-scenario+distro/    ESP geometry for each scenario/distro combination
  azl3/                  Original AZL3 configuration recipes
  azl4/                  AZL4 package/service/script overrides
  files/                 Image-local customization files
  scripts/               Distro-specific customization scripts
```

`tailor.yaml` is the workspace manifest and selects the Image Customizer
toolchain. Each image directory has an `image.yaml`. Fragments named
`by-<axis>/<value>.yaml` apply to matching cells; composite fragments such as
`by-scenario+distro/base+azl4.yaml` constrain several axes. Composite fragments
apply after single-axis fragments.

## VM model

The directory `trident-vm-testimage` declares the model name `vm`.

| Axis | Values |
|---|---|
| arch | amd64 |
| scenario | base, update |
| distro | azl3, azl4 |

Base cells produce QCOW2 images. Update cells produce COSI images, with the
pipeline generating two independently identified clones. AZL3 uses the existing
QEMU guest VHDX; AZL4 uses the AMD64 `minimal-os:4` catalog VHDX.
Inputs are staged under the repository's `artifacts` and `bin/RPMS` directories.

The pipeline adapts Tailor slugs to the existing artifact/file contracts:
`trident-vm-grub-testimage` for AZL3 and
`trident-vm-grub-azl4-testimage` for AZL4. QCOW2 artifact names have the
`image-` prefix and `-qcow2` suffix; update artifacts contain `_0.cosi` and
`_1.cosi` files.

## Shared assets and rendering

`$include` paths resolve relative to the image directory, including nested
includes. Paths are literal; select runtime/distro-specific includes through
fragments rather than interpolating the include filename.
Files can reference `../common/files`. Shared scripts are included YAML
snippets containing inline `content` and an explicit `/bin/bash` interpreter,
because Image Customizer script `path` entries cannot escape the image directory.
The `shellDollar` parameter preserves Bash `${...}` expressions through Tailor
interpolation; shared snippets spell them `${shellDollar}{...}`.

Pipeline jobs reset their local `tailor.yaml` from Git, select the dev container
when requested, and archive the effective manifest in `logs/tailor.yaml`.
No alternate manifest or Tailor Rust changes are required.

Inspect cells with `tailor --manifest tests/images/tailor/tailor.yaml matrix vm`.
