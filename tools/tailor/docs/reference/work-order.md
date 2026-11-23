# Work-order reference

A work order selects cells from the current workspace. It does not replace `tailor.yaml` or
`image.yaml`, configure the runtime, or freeze inputs across revisions.
Read it with `build -w PATH` or `build --work-order-env NAME`.

YAML and JSON use the same [structural schema](work-order.schema.json).
Workspace names, axes, values and selection results are validated separately.

## Fields and forms

| Field | Type | Meaning |
| --- | --- | --- |
| `schemaVersion` | integer | Required; exactly `1`. |
| `description` | string | Optional human metadata; no selection effect. |
| `images` | list of strings | Known image names for an image slice. |
| `select` | axis name to list of strings | Filters `images`: OR within an axis, AND across axes. Omitted means unrestricted. |
| `selections` | list of objects | Each object contains `images` and optional `select`, using the same rules. |
| `slugs` | list of strings | Exact requested cells added by union, never filtered by `select`. |
| `cells` | list of strings | Exclusive resolved form: exact requested slugs, not objects or implicit producers. |

Every form requires `schemaVersion`; `description` is always optional.

| Form | Required | Optional | Forbidden |
| --- | --- | --- | --- |
| Image slice | `images` | `select`, `slugs` | `selections`, `cells` |
| Multiple slices | `selections` | `slugs` | Top-level `images`, `select`, `cells` |
| Exact-only | `slugs` | None | `images`, `select`, `selections`, `cells` |
| Resolved | `cells` | None | `images`, `select`, `selections`, `slugs` |

Supplied lists and filters must be nonempty. Name/value lists contain unique nonblank strings.
Names match exactly; unknown fields and explicit `null` values fail. Different clauses may
overlap; the same cell is built once.

## Examples

An amd64 slice plus an arm64 exact addition:

```yaml
schemaVersion: 1
images: [gizmo]
select: {arch: [amd64]}
slugs: [gizmo_minimal_arm64_cosi]
```

Different filters:

```yaml
schemaVersion: 1
selections:
  - images: [gizmo, installer]
    select: {arch: [amd64]}
  - images: [qemu]
    select: {arch: [arm64]}
```

Exact-only:

```yaml
schemaVersion: 1
slugs: [gizmo_minimal_amd64_cosi]
```

Resolved:

```json
{"schemaVersion":1,"cells":["gizmo_minimal_amd64_cosi","gizmo_full_amd64_cosi"]}
```

Both exact forms still expand dependencies. Generation and execution must use the same workspace
revision and compatible Tailor version.

## Validation and dependencies

Every image, slug, axis and listed value must exist; valid entries cannot mask typos.
Each clause must select cells after skip handling. Errors identify the source and clause/field.

Naming an image overrides image-wide skip. Exact slugs and filters pinning a skipped fragment's
coordinate override fragment skip; unrelated filters do not. Base/image-input references add
their paired producer cells, including required skipped cells. `dependsOn` adds all normally
enabled prerequisite cells independent of consumer filters, retaining fragment skips.
Dependencies expand transitively and shared cells are deduplicated. Order-only outputs do not
become consumer fingerprint inputs.

See [Build work orders](../how-to/build-work-orders.md) and the [CLI reference](cli.md).
