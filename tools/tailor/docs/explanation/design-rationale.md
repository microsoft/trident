# Design rationale

This page explains *why* tailor is shaped the way it is — the decisions behind the model, the
alternatives that were rejected, and the consequences you live with as a result. For *how* the pieces
fit, see [Concepts](concepts.md) and the [Merge model](merge-model.md).

## Why a thin Image Customizer wrapper?

Image Customizer already owns image semantics: storage, packages, services, scripts, ISO/PXE behavior, and version-specific features. tailor deliberately does not mirror that schema. It owns only:

- loading workspace and image definitions,
- merging fragments,
- expanding matrices,
- resolving/pinning toolchains and base images,
- building the IC command line,
- running the IC container per cell.

The `config:` tree remains the user↔IC contract.

## Why manifests and matrices?

Raw IC usage requires repeating container invocations and copying near-identical YAML for every variant. tailor makes the repeated parts explicit and reusable: one shared `image.yaml`, small per-axis fragments, and a closed matrix of cells.

## Why logic in the file tree?

`by-release/4.0.yaml` is easier to audit than a large templated YAML file with nested conditionals. The directory name says when a fragment applies; the fragment body stays ordinary YAML.

## Why a small directive set?

Most composition is just deep-merge and list append. The directives exist only for the cases that need them:

- `$set` for deliberate overrides,
- `$replace` and `$remove` for list surgery,
- `$include` for shared YAML blocks.

This keeps authored files readable while still allowing exact control when needed.
