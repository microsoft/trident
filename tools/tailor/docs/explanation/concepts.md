# Concepts

## Workspace and standalone mode

A workspace has a `tailor.yaml` at its root. tailor finds it by walking up from the current directory. Member images are normally auto-discovered as immediate `*/image.yaml` files.

If no `tailor.yaml` exists, tailor treats the current directory's `image.yaml` as a standalone image and uses the built-in default IC toolchain unless the image defines one inline.

```mermaid
flowchart TD
  W["tailor.yaml"] --> A["app/image.yaml"]
  W --> D["db/image.yaml"]
  S["standalone image.yaml"] --> IC["built-in IC default"]
```

## Images

An image is the authoring unit. It has top-level tailor fields such as `name`, `base`, `matrix`, `outputs`, `params`, `rpmSources`, and `features`. Its `config:` tree is Image Customizer YAML and remains opaque to tailor.

## Matrices, axes, and cells

`matrix:` declares named axes. The cartesian product creates cells. Each cell is one Image Customizer invocation and one output artifact.

```yaml
matrix:
  edition: [lite, pro]
  arch: [amd64, arm64]
```

This creates four axis tuples before outputs are considered.

Most axes are opaque labels — any `[A-Za-z0-9.-]` string, meaningful only for partitioning the matrix,
`${axis}` interpolation, and `by-<axis>/` fragments. **`arch` is the one reserved, typed axis:** its
values are closed to `amd64`/`arm64` and it also drives the target platform and base-image resolution.
See [Target architectures](target-architectures.md).

## Slugs

A cell slug is:

```text
<image>_<axis values in matrix order>_<format>
```

For example:

```text
gizmo_lite_amd64_cosi
```

Axis values cannot contain `_`, because `_` separates slug components.

## Fragments

Fragments are per-axis deltas:

```text
gizmo/
  image.yaml
  by-edition/lite.yaml
  by-edition/pro.yaml
  by-arch/amd64.yaml
  by-arch/arm64.yaml
```

`by-edition/pro.yaml` applies only to cells where `edition=pro`. `by-feature/<name>.yaml` applies when the image lists that feature; features do not multiply the matrix.
