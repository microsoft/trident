# Generate a CI matrix

Use `matrix` to enumerate selected cells without running an engine:

```bash
tailor matrix appliance --format json
tailor matrix appliance --format slugs -s arch=amd64
```

JSON cells contain `image`, `slug`, `axes`, `format`, and (for a catalogue-backed cell)
`baseImage`. Build one with `tailor build <slug>` or `tailor build <image> --cell <slug>`.
In a multi-image workspace, bare `--cell` may fail on an unrelated image.

For Azure DevOps, generate the matrix object:

```bash
tailor matrix appliance --format ado
tailor matrix appliance --ado BUILD_MATRIX
```

The first emits a JSON job matrix. The second wraps it in one ADO output-variable line and
fails on an empty selection. Legs carry scalar JSON `TAILOR_WORK_ORDER`; consume it with
`tailor build --work-order-env TAILOR_WORK_ORDER`, not the sanitized leg key.
Dependencies stay together automatically. `--ado` and `--format` cannot be combined.

Add `--max-jobs N` to pack whole components into at most N jobs. This differs from ADO's
`maxParallel`; cells remain serial within a job. JSON/slugs inventories are unchanged.
Existing per-cell ADO templates must migrate. See [Build work orders](build-work-orders.md).

Use `tailor export --check` separately if the pipeline consumes committed, rendered IC YAML;
the matrix command only enumerates cells. See [Export configs](export-configs-for-a-pipeline.md)
and the [CLI reference](../reference/cli.md).
