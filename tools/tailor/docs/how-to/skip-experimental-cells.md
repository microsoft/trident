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

To skip just one matrix value, set `skip` in its fragment:

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
by an explicitly requested consumer are selected even when their producer cells are skipped.

See [Image definition](../reference/image-yaml.md#fragments).
