# COSI metadata schema samples

Sample `metadata.json` documents used to guard the COSI metadata JSON Schema
(`docs/Reference/Composable-OS-Image/cosi-metadata-v1.2.schema.json`).

They are validated on every pull request by the
`.github/workflows/cosi-schema-validation.yaml` gate:

- `valid/` — complete, well-formed COSI metadata that **MUST** validate against
  the schema. These are the strict-JSON counterparts of the illustrative
  (comment-annotated) examples in `docs/Reference/Composable-OS-Image.md`. If a
  schema change makes one of these fail, either the change is wrong or the
  sample needs updating.
- `invalid/` — documents that **MUST** be rejected, each violating exactly one
  constraint (missing required field, bad enum, wrong version, out-of-range
  value, malformed pattern). These prove the schema actually constrains its
  input rather than accepting anything.

## Adding a sample

Drop a `.json` file in `valid/` or `invalid/` as appropriate. Keep each
`invalid/` sample minimal: start from a valid document and introduce a single
violation, so the reason it fails is unambiguous.
