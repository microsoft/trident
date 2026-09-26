# Cross-arch building

tailor targets `amd64` by default. Build `arm64` by **declaring** it — nothing is inferred from the
host, so a workspace builds the same set on any machine.

## Build one arm64 image

Add an `arch` axis to the image:

```yaml
# image.yaml
name: gizmo
matrix:
  arch: [arm64]
base:
  oci:
    uri: "registry.example/gizmo/base:edge"
    platform: "linux/${arch}"      # ${arch} → linux/arm64
outputs:
  - format: cosi
config:
  os:
    hostname: gizmo
```

```sh
tailor build gizmo          # → gizmo_arm64_cosi.cosi
```

## Build both arches

List both values; tailor expands one cell per arch:

```yaml
matrix:
  arch: [amd64, arm64]       # → gizmo_amd64_cosi, gizmo_arm64_cosi
base:
  path: ./bases/gizmo-${arch}.img
```

## Amd64 is the default

With no `arch` axis and no workspace override, an image builds a single `amd64` cell:

```yaml
name: gizmo
base:
  path: ./bases/gizmo.img      # → gizmo_amd64_cosi
```

## Give a local base its own arch

A local `path` base can declare its own `arch`, which supplies the cell arch when there is no `arch`
axis:

```yaml
# image.yaml
base:
  path: ./bases/gizmo-arm64.img
  arch: arm64                  # this image builds one arm64 cell
```

A `baseImages:` catalogue slot's `arch` works the same way for `base: { ref: <name> }`.

## Platform must match the arch

The `arch` component of an `oci.platform` must equal the cell's arch. Always write `linux/${arch}` so
each cell pulls its own manifest; a fixed `platform: linux/arm64` on an `amd64` cell fails at
`validate` before any pull. `path` and `azureLinux` bases declare no arch, so the cell arch decides.

See [Target architectures](../explanation/target-architectures.md) for the full model.
