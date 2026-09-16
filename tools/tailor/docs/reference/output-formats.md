# Output formats

`outputs:` is a list of output specs. tailor creates one artifact per selected cell × output. Each
spec's `format` is **required**, and an unknown format is rejected at validate time.

```yaml
outputs:
  - format: cosi
```

| Format | Artifact extension/name |
| --- | --- |
| `cosi` | `.cosi` |
| `vhd` | `.vhd` |
| `vhd-fixed` | `.vhd` |
| `vhdx` | `.vhdx` |
| `qcow2` | `.qcow2` |
| `raw` | `.raw` |
| `iso` | `.iso` |
| `pxe-dir` | directory named as the cell slug |
| `pxe-tar` | `.tar.gz` |
| `baremetal-image` | `.raw` |

Optional output fields:

| Field | Meaning |
| --- | --- |
| `cosiCompressionLevel` | COSI compression level. tailor passes IC `--cosi-compression-level` when set. |
| `compression` | Post-build compression tailor applies to the artifact (`zstd` ⇒ `<slug>.<ext>.zst`). Not an IC feature; see below. |
| `name` | Optional `${...}` template for the output basename. |

### Compression

`compression: zstd` makes tailor compress the finished artifact: IC writes the raw image, then tailor
streams it through zstd and publishes `<slug>.<ext>.zst`, dropping the uncompressed original.

Only the raw disk-image formats support it — `vhd`, `vhd-fixed`, `vhdx`, `qcow2`, `raw`,
`baremetal-image`. It is rejected on `cosi` (IC already compresses it via `cosiCompressionLevel`),
`iso` (would break bootability), and `pxe-dir`/`pxe-tar` (a directory / an already-gzipped tar).

```yaml
outputs:
  - format: vhd-fixed
    compression: zstd     # → <slug>.vhd.zst
```

Use `$replace` in a fragment when you want to swap inherited outputs instead of appending another output.
