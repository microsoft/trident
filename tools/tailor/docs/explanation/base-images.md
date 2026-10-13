# Base images

A base image is the OS image Image Customizer modifies. tailor supports five sources. Registry
images resolve to digests for a run; file and workspace-image sources consume local artifacts.

## Registry bases vs. file bases

| Kind | Who supplies it | Build input | Portability |
| --- | --- | --- | --- |
| `oci`, `azureLinux` | Image Customizer, at build time | resolved registry digest (`--image`) | requires IC's `input-image-oci` preview flag and a cache dir |
| `path` | you | local file (`--image-file`) | requires the same file at the declared path on each machine |
| `ref` (catalogue) | `tailor bases download` **or** a CI feed | local file (`--image-file`) | requires the same file in the named slot |
| `image` | another workspace image | paired producer artifact (`--image-file`) | requires the producer build; see [dependent images](../how-to/build-dependent-images.md) |

The `oci`/`azureLinux` kinds are useful for dev builds straight from a registry, but may require
registry access during build and can resolve a newer digest on a later run. A `path` base avoids
that, but every
image repeats the same `../../../artifacts/<name>.vhdx`, and the `../` count breaks when the layout
moves.

## Change detection

For local file bases (`path` and catalogue `ref` slots), tailor computes an **XXH3-128** content
hash plus the file size before planning. This hash is only for rebuild/change detection; it is not a
security or provenance digest, and local files are not pinned in `tailor.lock`. Supply identical
bytes separately on each builder; a matching path alone is no provenance guarantee.

Large bases are cached on the build path. After the first hash, `tailor build` stores a versioned
entry under `<output>/.tailor/base-hashes/` keyed by the base path and guarded by the stored absolute
path, size, and mtime. If the size and mtime still match, the cached XXH3-128 value is reused and
the base file is not re-read (run with `-v` to see a `reusing cached hash, skipping read` line); if
anything is missing, malformed, unwritable, or changed, tailor just hashes the file again.

`tailor build --force` rebuilds even when a stamp says the artifact is current; it does **not**
disable the base hash cache. An unchanged base may still reuse its cached hash.

## The catalogue model

A **base-image catalogue** (`baseImages:` in `tailor.yaml`) resolves both problems. Each named **slot**
is a local file path plus, optionally, the remote source it came from. The build only ever sees the
**file**; only `tailor bases download` reads a slot's `source`. The path lives once; images reference
it by name with `base: { ref: <name> }`.

> It is like a local OCI cache, but the image depends on the cached *file* rather than the logical OCI
> image. The slot is the cache entry; `download` fills it; the image depends on the slot.

The payoff is that local dev and CI run the **same build**. They differ only in who fills the slot:

- **Local dev** — `tailor bases download` pulls from MCR/OCI into the slot path (idempotent).
- **Pipeline** — an out-of-band feed step drops the same files at the same paths; `tailor bases verify`
  asserts they arrived. No build-time pull, no preview feature.

## Arch reconciliation

A slot may declare its `arch` (`amd64`/`arm64`) — the same vocabulary as the `arch` axis, not a
`linux/...` platform string. It drives the pull platform and reconciles with the referencing cell: if
both are set they must agree, either fills the other, and a conflict is a `validate`-time error. This
is why per-arch local bases are modeled as arch-specific slots (e.g. `core_arm64`) swapped in by a
`by-arch/` fragment. See [Target architectures](target-architectures.md).

## What stays explicit

`validate` checks slot **names** offline, so a typo fails fast on a fresh checkout without the files
present. A missing file surfaces only when the build (or `tailor bases verify`) needs it, with a hint
to run `tailor bases download`. Catalogue-backed cells expose `baseImage: <name>` in `tailor matrix`,
making the cell→base dependency machine-readable for CI.

See [Use a base-image catalogue](../how-to/use-a-base-image-catalogue.md),
[`baseImages` reference](../reference/tailor-yaml.md), and [`image.yaml` base sources](../reference/image-yaml.md).
