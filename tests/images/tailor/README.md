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

## Host test-image model

```text
trident-testimage/
  image.yaml          AMD64/base/AZL4 matrix and COSI output
  base-azl4.yaml       Host test-image recipe
  scripts/            Host-specific post-customization
```

The `trident-testimage` model produces the AZL4 GRUB host COSI image.
It reuses `common` customization assets and the staged AZL4 RPMs/base.
The native host job publishes two clones under the existing
`trident-azl4-testimage` artifact name. It does not stage VM SSH keys:
the installer/Host Configuration creates the test user.
Image production is separate from test selection and adoption.

## Installer and MOS models

```text
trident-installer/
  image.yaml              AMD64/base, host/container, AZL3/AZL4 matrix
  azl3/                   Original host/container recipes shared by both distros
  runtime/                Runtime-specific files and customization scripts
  by-runtime/             Recipe and optional Trident RPM selection
  by-distro/              AZL4 base, preview feature and ESP geometry
  by-runtime+distro/      AZL4 runtime-specific package/service selection
  azl4/                   Package and service list fragments
trident-mos/
  image.yaml              AMD64/base, AZL3/AZL4 matrix
  azl3.yaml               Original MOS recipe
  by-distro/              AZL4 customization overrides
  files/                  ISO placeholders and boot configuration
  post-install.sh         MOS customization
```

All six cells produce one ISO each. Both models use the existing AZL3 baremetal
VHDX or the AZL4 catalog base. The host installer uses the matching Trident RPMs;
the container installer and MOS do not. MOS requires the built `rcp-agent` from
the `go-tools` artifact at `bin/rcp-agent`.
AZL4 ISO recipes install `dracut-live` explicitly because the minimal catalog
base does not include the live-boot initramfs modules.

The build-only layer keeps legacy installer stages and artifacts in place.
Native artifacts have a `tailor-` prefix, while their contained ISO filenames
retain the legacy names. Adoption is a separate pipeline change.
Split/direct-streaming installers, ARM64 images and azl-installer remain on
their existing build paths.

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
