# `image.yaml` reference

An image definition lives in an `image.yaml`. The top level belongs to tailor. The `config:` value is opaque Image Customizer YAML: tailor merges it structurally and passes it to IC.

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `name` | string | yes | Image id used by CLI and slugs. Use `[A-Za-z0-9.-]+`; `_` is reserved as the slug separator. |
| `skip` | bool | no | Default `false`. When `true`, the image is excluded from **bulk** selection — any command run with no image named (`build`, `matrix`, `slugs`, `validate`, `render`, `export`, `clean`, …), so a pipeline that runs those over all images never picks it up. Build it by naming it explicitly (`tailor build <name>`); `tailor list` marks it `(skip)`. Useful for an experimental config that isn't ready to build in CI. **Also valid in a `by-<axis>/<value>.yaml` fragment** — there it drops every cell that picks that value from bulk selection, unless the run pins that value (`-s <axis>=<value>`) or names the cell (`--cell <slug>`); a non-pinning selector like `-s arch=amd64` does *not* resurrect it. Fragment `skip` merges last-wins (a more-specific fragment may set `skip: false`). |
| `toolchain` | string or `{name, container, version?, tag?, pull?}` | no | Workspace toolchain name or inline standalone toolchain. Defaults to workspace default or built-in `latest`; `pull` defaults to `missing`. |
| `toolsDir` | `{source}` | no | Tailor-managed IC `--tools-dir`. `source` is a `toolsDirSources` name or inline `{container, tag?, pull?}`. Always bound writable as a per-cell copy under `runtime.buildDirBase` (which defaults to `<output>/.tailor/build`). |
| `matrix` | ordered map `axis: [values]` | no | User-defined axes; their cartesian product is the candidate cells. Omit for one cell. Declaration order controls slug order and fragment precedence — order axes widest → most-specific (so `arch` is first). |
| `selectors` | `{ include?, exclude? }` | no | Which cells of the `matrix:` product to build. Lists of **selectors** (sub-cubes); `include` is an allowlist, `exclude` a denylist. Requires `matrix:`; omitted ⇒ the full product. |
| `outputs` | list of output specs | no | Defaults from workspace or built-in `cosi`. One artifact per cell × output. |
| `base` | one of `path`, `oci`, `azureLinux`, `ref`, `image` | conditional | Exactly one base resolves per cell. `ref: <name>` references a `baseImages:` slot; `image: <name>` bases on another workspace image's output (see [Inter-image dependencies](#inter-image-dependencies)). |
| `features` | string list | no | Enables matching `by-feature/<name>.yaml` fragments. Does not multiply cells. |
| `params` | scalar map | no | Values interpolated into `config:` strings with `${name}`. Params may reference other params. |
| `rpmSources` | path list | no | Each path is a directory of RPMs or a `.repo` file; passed as IC `--rpm-source`. |
| `operation` | `customize` or `convert` | no | Default: `customize`. |
| `signing` | `true` or profile id | no | Opt in to the signed-image pipeline. `true` ⇒ the workspace `signing.default` profile; a string ⇒ that named profile; omitted ⇒ unsigned. See [Sign an image](../how-to/sign-an-image.md). |
| `extraDependencies` | path list | no | Extra files/directories to hash for incremental checks; use for IC-config-referenced assets. |
| `dependsOn` | string list | no | Order-only build dependencies on other workspace images. See [Inter-image dependencies](#inter-image-dependencies). |
| `inputs` | list of `{name, image, output?, cell?}` | no | Named producer artifacts embedded in `config:` via `${inputs.<name>}`. See [Embedding an artifact with `inputs:`](#embedding-an-artifact-with-inputs). |
| `extraParams` | list of `{param, value?}` | no | Extra Image Customizer command-line flags, appended verbatim after every flag tailor manages. For experimental/non-standard IC builds. See [Extra params](#extra-params). |
| `config` | mapping or path string | conditional | Required for `customize`, forbidden for `convert`. Opaque IC config. |

## Tools dir

Use `toolsDir:` when the image needs an external package-manager userspace for IC operations. The
`source` is either a name from workspace `toolsDirSources:` or an inline container source. The
tools-dir is always bound **writable** as a per-cell disposable copy under `runtime.buildDirBase`
(IC rewrites `resolv.conf` inside the tools chroot during package operations, so a read-only bind
cannot work). `runtime.buildDirBase` defaults to `<output>/.tailor/build`, so no configuration is
needed; set it to override the location. Inline sources
accept the same `pull: always | missing | never` policy as workspace `toolsDirSources`; local-only
images without a `RepoDigest` run by image `Id` and are not lockable. The inline IC
`config.previewFeatures` list must include `tools-dir`.

```yaml
toolsDir:
  source: base

config:
  previewFeatures:
    - tools-dir
```

```yaml
toolsDir:
  source:
    container: quay.io/fedora/fedora
    tag: "42"
    pull: missing

config:
  previewFeatures:
    - tools-dir
```

tailor exports the source container to `runtime.imageCacheDir/tools-dirs/<digest>` and passes the
translated `/host/...` path to customize passes only. It never emits `--tools-dir /`, and convert or
inject-files passes do not receive the flag.

## Extra params

`extraParams:` passes extra flags straight through to Image Customizer, for experimental or
non-standard IC builds that expose options tailor does not model. Each entry is a `param` flag with
an optional `value`, joined with a single `=`:

```yaml
extraParams:
  - param: --experimental-thing   # → argv token: --experimental-thing=fast
    value: fast
  - param: --debug-stage          # → bare flag: --debug-stage
```

The flags are appended **verbatim, after every flag tailor manages**, to the `customize`/`convert`
invocation (and to the signed-build `customize` pass). They are part of the incremental fingerprint,
so changing a `param` or `value` rebuilds the affected cells.

`extraParams` is **mergeable like `rpmSources`**: it is concatenated across the base document and
every matched fragment, base → most-specific, so a `by-<axis>/<value>.yaml` fragment can add a flag
for just its cells.

A flag tailor already emits itself (`--config-file`, `--build-dir`, `--output-image-file`,
`--output-image-format`, `--rpm-source`, `--tools-dir`, `--image`, `--image-file`,
`--image-cache-dir`, `--cosi-compression-level`, and the `--log-*` flags) is **rejected** — tailor
owns those, and a duplicate would fight its own argument. Use the modelled field instead.

## Matrix

`matrix:` declares the axes; their cartesian product (in declaration order) is the candidate cells.
The optional `selectors:` block chooses which of those cells to actually build.

```yaml
matrix:                   # axes only — order widest → most-specific (arch first)
  arch: [amd64, arm64]
  edition: [lite, pro]
  channel: [stable, edge]

selectors:                # omit entirely ⇒ build the full product
  include:                # allowlist: keep cells matched by any selector (full product if absent)
    - { arch: amd64 }                        # every amd64 cell
    - { arch: arm64, edition: lite }         # plus the lite arm64 cells (channel expands)
  exclude:                # denylist: then drop cells matched by any selector (exclude wins)
    - { edition: pro, channel: [stable, edge] }   # a value may be a list
```

A **selector** is a partial assignment over the axes: each axis is pinned to a value or a **list** of
values, and **omitted axes match every value**. The final cell set is the union of the `include`
selectors (or the full product when `include` is empty), minus the union of the `exclude` selectors.

Axes are closed: every selector and `by-<axis>/<value>.yaml` fragment path must use declared axis
names and values. Selecting zero cells from a non-empty matrix is an error.

## Architectures

`arch` is the one **reserved** axis. Its values are closed to `amd64` and `arm64`, and the cell's
arch drives `--platform linux/<arch>`, per-arch base selection, the slug, and `${arch}`. Each cell
has exactly one arch, resolved in this order:

1. the `arch` matrix axis, one cell per value (`matrix.arch: [amd64, arm64]`);
2. else the **base image's own arch** — a `baseImages:` slot's `arch`, a local `path` base's `arch`,
   or an `oci.platform`'s arch component;
3. else **`amd64`**.

There is no `architectures:` field — neither per-image nor a workspace default. Declare a non-default
arch with the axis, or let the base image's own arch supply it. The default is fixed at `amd64` and
never the host arch, so a workspace builds the same set everywhere. See
[Target architectures](../explanation/target-architectures.md)
and [Cross-arch building](../how-to/cross-arch-building.md).

## Fragments

Per-cell deltas live in `by-*/` files whose **path** is the condition — no inline `match:` needed. A
fragment applies to a cell when its path predicate holds:

| Path | Applies when | Kind |
| --- | --- | --- |
| `by-arch/amd64.yaml` | `arch == amd64` | single axis, single value |
| `by-mode/dev+test.yaml` | `mode ∈ {dev, test}` | single axis, **disjunction** |
| `by-boot+verity/uki+root.yaml` | `boot == uki` **and** `verity == root` | multi-axis **conjunction** |
| `by-feature/<name>.yaml` | the feature is enabled | feature flag |

`+` joins axes in the directory and values in the file. A directory naming **one** axis lets the file list
several values (a disjunction, in the axis's declared value order); a directory naming **several** axes
takes exactly one value per axis, positionally, with the axes in matrix-declared order. `image.yaml` is the
base (applies to every cell).

Apply order is merge precedence (later wins for scalars, extends lists). Fragments are sorted by: **arity**
(more axes apply later — a composite refines the singles it builds on), then **axis-declaration order**
(cross-axis precedence follows the matrix), then **breadth** (a broader disjunction applies before a
narrower single value on the same axis, so the more specific one wins). Run `tailor explain <image> --cell
<slug>` to print the exact merge order for a cell. See [Merge directives](directives.md) and
[Merge model](../explanation/merge-model.md) for the full model.

## Base sources

```yaml
base:
  path: ./bases/gizmo-amd64.img
```

```yaml
base:
  oci:
    uri: "registry.example/gizmo/base:edge"
    platform: "linux/${arch}"
```

```yaml
base:
  azureLinux:
    version: "3.0"
    variant: minimal-os
```

```yaml
base:
  ref: baremetal          # a named slot in tailor.yaml `baseImages:`
```

For an `oci` or `azureLinux` base, tailor resolves the registry digest and passes IC a digest-pinned
`--image oci:<repo>@sha256:…` (so the build is reproducible). Image Customizer downloads OCI input
images behind a **preview feature**, and tailor never edits your IC `config:`, so you must enable it
yourself in the image's `config:`:

```yaml
config:
  previewFeatures:
    - input-image-oci
```

Registry bases also need an image cache directory; tailor defaults `runtime.imageCacheDir` to
`<workspace>/.tailor/cache` when you set none (see [tailor.yaml](tailor-yaml.md)).

The `arch` component of an `oci.platform` must match the cell's arch, so `linux/${arch}` is the safe
spelling. Pinning a fixed `platform: linux/arm64` on an amd64 cell is a validate-time error.

A `ref:` base references a named slot from the workspace `baseImages:` catalogue and resolves to
that slot's local file (the path lives once, in `tailor.yaml`). Use it for the file-based, registry-pull-free
flow Trident needs — see [`baseImages` in tailor.yaml](tailor-yaml.md), [Use a base-image catalogue](../how-to/use-a-base-image-catalogue.md),
and [Base images](../explanation/base-images.md).

## Inter-image dependencies

One image can build on another image in the same workspace. Today this is expressed as a **base**:

```yaml
# derived/image.yaml — customize another workspace image's output further
base:
  image: base-os          # a member image name
  output: raw             # which producer output (format name); optional if it has one output
  cell: { flavor: min }   # pin producer axes this image doesn't share (see below)
```

`base: { image }` resolves, **per cell**, to the producer's published artifact for the paired cell,
then behaves exactly like a `path` base. tailor builds the producer first: a single `tailor build`
orders the images topologically, and `tailor build derived` builds `base-os` before `derived`. A
producer rebuild re-fingerprints its consumers (their base content-hash changes), so incremental
builds stay correct. A dependency **cycle** is a hard error.

**Cell pairing.** For each consuming cell, the producer cell is chosen by matching axes the two share
(canonically `arch` — an `arm64` consumer pairs with the producer's `arm64` output), plus any
`cell:` pins for producer axes the consumer lacks. An unpinned producer-only axis is ambiguous (an
error); a coordinate that names no producer cell, an unknown output format, or a bad pin are errors —
all surfaced by `tailor validate`.

**`output`** is the producer output's **format name** (e.g. `raw`, `vhd-fixed`, `cosi`), not a file
extension; tailor derives the extension (and appends `.zst` if that output is compressed). It is
optional only when the producer declares a single output.

`dependsOn: [<image>, …]` declares an **order-only** dependency: the listed images build first, but
they contribute nothing to this image's fingerprint. Use it when an image must run after another but
references none of its output. (An `image` base or input already implies the edge — don't restate it.)

### Embedding an artifact with `inputs:`

To consume a producer's artifact *inside* `config:` (e.g. as an IC `additionalFiles` source or a
local `rpmSources` entry), declare it in `inputs:` and reference it by name as `${inputs.<name>}`:

```yaml
inputs:
  - name: payload            # the interpolation key
    image: installer-payload # producer image
    output: cosi             # format name; optional if single-output
    cell: { flavor: min }    # pin producer axes this image doesn't share

config:
  os:
    additionalFiles:
      - source: "${inputs.payload}"   # → the resolved producer artifact path
        destination: /images/payload.cosi
```

`${inputs.<name>}` is substituted with the resolved producer artifact path (same per-cell pairing and
`output`/`cell` rules as `base: { image }` above), and the artifact is content-hashed into the
fingerprint — so a producer rebuild rebuilds this image, and you never hand-write a
`../…/artifacts/…` path. A `${inputs.<name>}` that names no declared input is an error. `inputs`
entries whose kind is `image` add a build-order edge; each input `name` is unique per image.

## Output spec

```yaml
outputs:
  - format: cosi
    cosiCompressionLevel: 6
    name: "${name}-${arch}"
  - format: vhd-fixed
    compression: zstd      # tailor compresses the artifact → <slug>.vhd.zst
```

`format` is required. `cosiCompressionLevel`, `compression`, and `name` are optional.

### `compression`

Post-build compression tailor applies to the artifact. Image Customizer writes the raw image (e.g.
`<slug>.vhd`); tailor then streams it through the codec and publishes `<slug>.vhd.zst`, removing the
uncompressed original. This is a **tailor** step, not an IC feature, and is independent of
`cosiCompressionLevel` (which is IC's own COSI compression).

| Codec | Suffix |
| --- | --- |
| `zstd` | `.zst` |

`compression` is **invalid** for `cosi` (already compressed by IC), `iso` (compressing the image
breaks bootability), and the `pxe-*` outputs (a directory / an already-gzipped tar) — `validate`
rejects those combinations. It applies to the raw disk-image formats: `vhd`, `vhd-fixed`, `vhdx`,
`qcow2`, `raw`, `baremetal-image`. Changing it re-fingerprints the cell, so the artifact rebuilds.

