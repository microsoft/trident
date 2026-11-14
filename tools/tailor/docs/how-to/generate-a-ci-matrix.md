# Generate a CI matrix

Use `matrix` to enumerate selected cells without running an engine:

```bash
tailor matrix appliance --format json
tailor matrix appliance --format slugs -s arch=amd64
```

JSON cells contain `image`, `slug`, `axes`, `format`, and (for a catalogue-backed cell)
`baseImage`. A slug can be fed to `tailor build <slug>` or `tailor build --cell <slug>`.

For Azure DevOps, generate the matrix object:

```bash
tailor matrix appliance --format ado
tailor matrix appliance --ado BUILD_MATRIX
```

The first emits a bare JSON object. The second emits an ADO `##vso[task.setvariable]` line
for `BUILD_MATRIX` and fails on an empty selection. Each leg contains string variables
`image`, `slug`, `format`, optionally `baseImage`, and `axis_<axis>` for each coordinate.
ADO leg keys are sanitized (and disambiguated if needed); **use the leg's `slug` value** when
invoking `tailor build`, not its sanitized key. `--ado` and `--format` cannot be combined.

Use `tailor export --check` separately if the pipeline consumes committed, rendered IC YAML;
the matrix command only enumerates cells. See [Export configs](export-configs-for-a-pipeline.md)
and the [CLI reference](../reference/cli.md).
