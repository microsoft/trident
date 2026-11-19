# Skip experimental cells in bulk runs

Exclude an entire image from no-argument bulk commands:

```yaml
# experimental/image.yaml
name: experimental
skip: true
base: { path: ./input.img }
config:
  os:
    hostname: experimental
```

`tailor build` and `tailor matrix` omit it, but `tailor build experimental --dry-run` selects
it explicitly. `tailor list` marks the image `(skip)`.

To skip just one matrix value instead, remove the image-wide `skip: true` and declare the axis:

```yaml
# experimental/image.yaml
name: experimental
base: { path: ./input.img }
matrix:
  channel: [stable, edge]
config:
  os:
    hostname: experimental
```

Then set `skip` in the matching fragment:

```yaml
# experimental/by-channel/edge.yaml
skip: true
```

Bulk selection drops edge cells. Request them by pinning the skipped coordinate:

```bash
tailor build experimental -s channel=edge --dry-run
```

A selector on a different axis alone does not unskip edge. A more-specific fragment may use
`skip: false` to override the skipped value. This controls *selection*, not whether the YAML is
valid; `tailor validate experimental -s channel=edge` still renders that cell. Dependencies needed
through `base: {image}` or `inputs:` are selected even when their producer cells are skipped.
Order-only `dependsOn` prerequisites retain fragment skips.

See [Image definition](../reference/image-yaml.md#fragments).
