# Override a base per axis

Put the shared base in `image.yaml`, then override it from the fragment for the axis value that needs a different source.

```yaml
# image.yaml
name: gizmo
matrix:
  channel: [stable, edge]
  arch: [amd64, arm64]
base:
  path: ./bases/gizmo-${arch}.img
outputs:
  - format: cosi
config:
  os:
    hostname: gizmo
```

Override the edge channel with `$set`:

```yaml
# by-channel/edge.yaml
base:
  $set:
    oci:
      uri: "registry.example/gizmo/base:edge"
      platform: "linux/${arch}"
```

Exactly one base must resolve for every cell. Base sources are one of:

- `path: ./local.img`
- `oci: { uri: "registry/name:tag", platform: "linux/${arch}" }`
- `azureLinux: { version: "3.0", variant: minimal-os }`
- `ref: <name>` — a named slot from the workspace [`baseImages:` catalogue](../reference/tailor-yaml.md)

Use `$set` for a deliberate scalar or whole-value override; otherwise conflicting base assignments are errors.

For per-arch **local files** define one catalogue slot per arch and swap the slot in a `by-arch/` fragment;
see [Use a base-image catalogue](use-a-base-image-catalogue.md).
