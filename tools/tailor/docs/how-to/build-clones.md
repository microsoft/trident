# Build clones of a cell

Use `--clones` when one selected cell should produce several distinct artifacts for machines that
must not share generated identity.

## Build several clones

```bash
tailor build gizmo --clones 3
```

`--clones` defaults to `1`. With `--clones 3`, tailor builds each selected cell three times. For a
cell slug like `gizmo_pro_amd64_stable_cosi`, the published output slugs are:

- `gizmo_pro_amd64_stable_cosi_clone0`
- `gizmo_pro_amd64_stable_cosi_clone1`
- `gizmo_pro_amd64_stable_cosi_clone2`

The artifacts share meaningful content, but differ in incidental details such as fresh UUIDs and
timestamps. Clone builds always run; incremental up-to-date stamps are ignored for clones.

Related: [Select and build one cell](select-and-build-one-cell.md); see the
[`tailor build` CLI reference](../reference/cli.md).
