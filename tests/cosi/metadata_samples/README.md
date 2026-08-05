# COSI metadata schema samples

Sample `metadata.json` documents used to guard the COSI metadata JSON Schemas in
`docs/Reference/Composable-OS-Image/` (`cosi-metadata-v<MAJOR>.<MINOR>.schema.json`).

They are validated on every pull request by the
`.github/workflows/cosi-schema-validation.yaml` gate.

## Layout

Samples are grouped per schema revision:

```
tests/cosi/metadata_samples/
  v1.2/
    valid/     # documents that MUST validate against cosi-metadata-v1.2.schema.json
    invalid/   # documents that MUST be rejected by cosi-metadata-v1.2.schema.json
```

The gate pairs each `docs/.../cosi-metadata-v<X.Y>.schema.json` with the
`tests/cosi/metadata_samples/v<X.Y>/` directory and, for that revision:

- meta-validates the schema is a valid JSON Schema (draft 2020-12);
- asserts every `valid/*.json` conforms;
- asserts every `invalid/*.json` is rejected (each violating exactly one
  constraint, so the schema is proven to actually constrain its input).

## Adding a new revision (e.g. v1.3)

1. Add `docs/Reference/Composable-OS-Image/cosi-metadata-v1.3.schema.json`.
2. Add `tests/cosi/metadata_samples/v1.3/valid/` and `.../invalid/` samples.

The workflow discovers versions from the schema filenames, so no workflow change
is needed.

## Sample conventions

- `valid/` samples are complete, strict-JSON counterparts of the illustrative
  (comment-annotated) examples in `docs/Reference/Composable-OS-Image.md`.
- Keep each `invalid/` sample minimal: start from a valid document and introduce
  a single violation, so the reason it fails is unambiguous.
