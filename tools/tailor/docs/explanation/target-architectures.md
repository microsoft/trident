# Architectures

`arch` is tailor's one **reserved** matrix axis. It looks like any other axis, but it is *typed*: its
values are closed to `amd64` and `arm64`, and the cell's arch is wired straight into the build.

> Most axes only select fragments and interpolate into strings. `arch` does that **and** controls the
> target platform and base-image resolution — so it needs the extra rules on this page.

## The reserved axis

Every other axis (`edition`, `channel`, `variant`, …) is an opaque label — any `[A-Za-z0-9.-]`
string, meaningful only for partitioning the matrix, `${axis}` interpolation, `by-axis/` fragments,
and `--select`. `arch` is all of that **plus** real semantics tailor reaches into by name:

- the container platform is **`--platform linux/<arch>`**;
- the base image is selected per arch (a `baseImages:` slot's `arch`, and the `oci.platform`);
- `arch` is always part of the slug;
- `${arch}` interpolates into config and base URIs.

So values are restricted: `amd64` or `arm64`. Anything else is an error.

## Every cell has exactly one arch

A cell's **effective arch** is resolved in this order:

1. the **`arch` matrix axis** — one cell per value;
2. else the **base image's own arch** — a `baseImages:` slot's `arch`, a local `path` base's `arch`,
   or an `oci.platform`'s arch component;
3. else the built-in default **`amd64`**.

The default is fixed at `amd64` — it is **not** the host arch. A build on an arm64 host still targets
amd64 unless you declare otherwise, so a workspace produces the same set on every machine. To build
another arch, declare the `arch` axis, or let the base image's own arch supply it.

There is no `architectures:` field — neither per-image nor a workspace default. An image declares a
non-default arch through the `arch` axis, or inherits it from the base image it resolves to.

## Declaring arch with the axis

```yaml
# image.yaml
name: gizmo
matrix:
  arch: [amd64, arm64]      # two cells: gizmo_amd64_cosi, gizmo_arm64_cosi
base:
  path: ./bases/gizmo-${arch}.img
outputs:
  - format: cosi
```

With no `arch` axis and no base arch, `gizmo` builds one `amd64` cell. Order `arch` first in
the matrix so it sits widest in the slug and fragment precedence.

## `--platform` and the base

The cell arch is the single source of truth: `--platform linux/<arch>`, the base pull, the slug, and
`${arch}` all derive from it. A registry base is multi-arch — `platform: linux/${arch}` selects the
right manifest per cell. A fixed `platform: linux/arm64` on an amd64 cell would pull the wrong
manifest, so tailor rejects it at `validate` time: the `arch` component of `oci.platform` must equal
the cell arch. `path` and `azureLinux` bases declare no arch, so they never conflict — the cell arch
decides.

## The effective-arch matrix

When both the image arch (the axis) and a base-image arch (a catalogue slot's `arch`, or an
`oci.platform`'s arch component) are set, they must agree:

| image arch ↓ \ base arch → | _(unset)_ | `arm64` | `amd64` |
| --- | --- | --- | --- |
| **_(unset)_** | `amd64` | `arm64` | `amd64` |
| **`arm64`** | `arm64` | `arm64` | **error** |
| **`amd64`** | `amd64` | **error** | `amd64` |

Both unset → `amd64` (the fixed built-in default; there is no workspace-wide arch override); exactly one set fills in the other; both set must
agree, else it is a validate-time error naming the cell and the two arches.

See [Cross-arch building](../how-to/cross-arch-building.md), [image.yaml](../reference/image-yaml.md),
and [tailor.yaml](../reference/tailor-yaml.md).
