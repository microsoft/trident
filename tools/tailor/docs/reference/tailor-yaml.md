# `tailor.yaml` reference

`tailor.yaml` is the workspace manifest: it configures toolchains, runtime defaults, and image discovery. tailor finds it by walking up from the current directory.

```yaml
schemaVersion: 1

toolchains:
  default: ic
  entries:
    - name: ic
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      # version: "1.3.0"
      # tag: "1.3.0"
      # pull: missing

defaults:
  outputs:
    - format: cosi
```

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `schemaVersion` | integer | yes | Current value: `1`. tailor rejects a version newer than it supports. |
| `previewFeatures` | list of strings | no | Opt in to features not yet part of the stable 1.0 contract. Currently: `signing`. Using a gated feature (e.g. `signing:`) without listing it here is a hard error. |
| `toolchains.default` | string | yes | Default toolchain name for images that omit `toolchain:`. |
| `toolchains.entries` | list of `{name, container, version?, tag?, pull?}` | yes | Named toolchain definitions. Each `name` must be unique. `tag` defaults to `version`, else `latest`; `pull` defaults to `missing`. |
| `toolsDirSources` | list of `{name, container, tag?, pull?}` | no | Named tools-dir sources. Each `name` must be unique. `tag` defaults to `latest`; `pull` defaults to `missing`. Images opt in with `toolsDir.source`. |
| `runtime.engine` | enum | no | Container engine: `docker` (default), `podman`, or `auto`. See [Select a container engine](../how-to/select-a-container-engine.md). |
| `runtime.host` | string | no | Explicit engine endpoint (`unix://…`, a bare socket path, or `tcp://…`), overriding the engine default and `DOCKER_HOST` / `CONTAINER_HOST`. |
| `runtime.privileged` | bool | no | Default `true`; IC requires privileged container execution. |
| `runtime.mounts.hostRoot` | path | no | Default `/host`; namespace prefix for translated host paths. tailor no longer binds host `/` there. |
| `runtime.mounts.dev` | bool | no | Default `true`; bind `/dev:/dev`. |
| `runtime.mounts.extraPaths` | list of extra mount objects | no | Additional paths exposed under `hostRoot`. `access` defaults to `ro`; use `rw` only for explicit writable carve-outs. Relative paths resolve against the workspace root. |
| `runtime.buildDirBase` | path | no | Host filesystem base for per-cell IC build dirs (`<buildDirBase>/<slug>`). Omit to use the default under the output dir (`<output>/.tailor/build`). Must not be `/`, a system directory, or `$HOME`. |
| `runtime.logLevel` | enum | no | IC log level: `panic`, `fatal`, `error`, `warn`, `info`, `debug`, `trace`. |
| `runtime.logDir` | path | no | Directory for on-disk IC log files (one per cell). Off by default (logs stream to the console). Overridden, highest precedence first, by the `--log-dir` flag then the `TAILOR_LOG_DIR` environment variable. |
| `runtime.imageCacheDir` | path | no | Cache for registry base images. Default: `<workspace>/.tailor/cache`. Required by IC for `oci`/`azureLinux` bases — tailor supplies the default so they build out of the box. |
| `runtime.janitorImage` | `{container, tag?}` | no | Minimal image used for sudo-free ownership cleanup. Default: `mcr.microsoft.com/azurelinux/base/core:3.0`. |
| `signing.default` | string | no | Signing profile used when an image says `signing: true`. See [Sign an image](../how-to/sign-an-image.md). |
| `signing.profiles` | map of `{backend, …}` | no | Named signing profiles. `backend` is `local-test-ca` (optional `publishCaCert`: where to write the enrollable CA certificate), `keypair` (needs `key`+`cert`), or `azure-key-vault` (needs `vault`+`certificate`). |
| `defaults.outputs` | output list | no | Inherited by images without `outputs`. |
| `defaults.outputArtifacts` | enum | no | Default `output.artifacts` staging policy for images that don't set their own: `managed` (default — relocate the extracted artifacts to the output dir and keep them), `scratch` (treat as signing scratch: extract, then reclaim), or `strip` (drop the `output.artifacts` block so IC never extracts). A per-image `outputArtifacts` overrides this. |
| `export.outputDir` | path | cond | Committed directory for `tailor export` output (one `<slug>.yaml` per cell), relative to the workspace root. Required to run `tailor export` argument-free. See [Export](#export). |
| `export.scope` | enum | no | What `tailor export` emits. Defaults to `configsOnly` (the only value today); omit it. |
| `export.images` | list of strings | no | Restrict `tailor export` to a subset of images. Default: all images. |
| `baseImages` | list of `{name, path, arch?, source?}` | no | Base-image catalogue: named slots an image references with `base: { ref: <name> }`. Each `name` must be unique. See [base-image catalogue](#base-image-catalogue). |
| `images` | object | no | Omit to auto-discover every immediate `*/image.yaml`. |

Runtime mounts expose only the workspace (read-only), tailor-owned writable carve-outs, and declared
out-of-workspace inputs. The old whole-host `-v /:/host` bind is never emitted.

## Pull policy

Toolchains and tools-dir sources support `pull: always | missing | never`:

- `always` resolves the image from its registry and pulls before use.
- `missing` (default) uses a local image when present, otherwise resolves and pulls from the registry.
  If `tailor.lock` already pins a digest for the named source, the locked digest wins.
- `never` requires a locked digest or a local image and never contacts the registry.

Local images that expose a `RepoDigest` are lockable and run as `container@sha256:…`. Local-only
images without a `RepoDigest` run by their image `Id` and are intentionally omitted from
`tailor.lock`.

A local toolchain image serves the architecture it was built for. tailor reads that architecture
during resolution and **fails fast, before any container run,** when it cannot provide a selected
cell's arch (rather than letting Docker attempt a slow, doomed cross-arch pull):

```
error: toolchain `local-ic` local image is `amd64` but cell `gizmo_arm64_cosi` targets `arm64`;
       no local image for that arch and pull policy won't fetch it
```

`pull: never` with no local image and no locked digest fails the same way — never silently reaching a
registry. See [Use a locally-built Image Customizer image](../how-to/use-a-local-ic-image.md).

A toolchain entry with every optional key set:

```yaml
toolchains:
  default: ic
  entries:
    - name: ic
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      version: "1.3.0"     # optional semver metadata; used as the tag when `tag` is unset
      tag: "1.3.0"         # the registry tag actually pulled (defaults to `version`, else `latest`)
      pull: missing        # always | missing (default) | never
```

```yaml
runtime:
  buildDirBase: /mnt/tailor-build
  mounts:
    hostRoot: /host
    dev: true
    extraPaths:
      - path: /opt/shared-scripts
      - path: /data/scratch
        access: rw
```

## Tools-dir sources

`toolsDirSources:` is a list of named container root filesystems that tailor can export, cache, bind,
and pass to IC as `--tools-dir`. This is useful for sealed/minimal images whose target root does not
contain a package manager. The cache is tailor-owned under `runtime.imageCacheDir/tools-dirs/<digest>`
and is bound read-only by default. tailor never passes `--tools-dir /`.

```yaml
toolsDirSources:
  - name: base
    container: mcr.microsoft.com/azurelinux/base/core
    tag: "3.0"
    pull: missing
  - name: fedora
    container: quay.io/fedora/fedora
    tag: "42"
```

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `name` | string | yes | Unique source name used by image `toolsDir.source`. |
| `container` | string | yes | Container image whose flattened root filesystem becomes the tools dir. |
| `tag` | string | no | Registry tag. Defaults to `latest` when `container` has no tag or digest. |
| `pull` | `always`\|`missing`\|`never` | no | Pull policy. Defaults to `missing`; see [Pull policy](#pull-policy). |

Run `tailor lock` to pin named tools-dir source digests in `tailor.lock`. Local-only tools-dir
images without a registry digest are usable by local image `Id`, but are not lockable.

## Base-image catalogue

`baseImages:` is a list of named **slots**, each a local base-image file plus an optional remote source
`tailor bases download` pulls it from. An image references a slot by name with `base: { ref: <name> }`,
so the path lives once here instead of being repeated (with brittle `../` counts) in every image.

```yaml
baseImages:
  - name: baremetal
    path: bases/baremetal.vhdx      # build input + download output (workspace-root-relative)
    arch: amd64                     # amd64 | arm64; reconciles with the cell arch
    source:                         # optional: how `tailor bases download` fills the slot
      azureLinux:
        version: "3.0"
        variant: baremetal
  - name: core_arm64
    path: bases/core_arm64.vhdx
    arch: arm64
    source:
      oci:
        uri: registry.example/core:3.0
  - name: qemu
    path: bases/qemu.vhdx           # no source: filled out-of-band (e.g. a CI feed)
```

| Field | Type | Required | Notes |
| --- | --- | --- | --- |
| `name` | string | yes | Unique slot name used by `base: { ref: <name> }`. |
| `path` | path | yes | The slot file: build input **and** `download` output. Workspace-root-relative. |
| `arch` | `amd64`\|`arm64` | no | The base's architecture; reconciles with the referencing cell's arch. Absent ⇒ the cell decides. |
| `source` | `{oci}` or `{azureLinux}` | no | A remote source `download` pulls for `linux/<arch>`. Absent ⇒ pre-placed; `download` skips it. |

Fill slots with [`tailor bases download`](cli.md) and assert presence with `tailor bases verify`. See
[Use a base-image catalogue](../how-to/use-a-base-image-catalogue.md) and [Base images](../explanation/base-images.md).

## Export

The `export:` block configures [`tailor export`](cli.md), which renders each cell's merged Image
Customizer config to a committed directory so a pipeline can build the images without tailor.

```yaml
export:
  outputDir: rendered      # committed output dir, relative to the workspace root
  # scope: configsOnly     # optional; defaults to configsOnly (the only scope today)
  # images: [gadget]       # optional; default = all images
```

| Field | Type | Req | Meaning |
| --- | --- | --- | --- |
| `outputDir` | path | yes | Committed directory receiving one `<slug>.yaml` per cell. Relative to the workspace root. |
| `scope` | enum | no | What to emit; defaults to `configsOnly`. An enum so future scopes can be added without a breaking change — omit it today. |
| `images` | list of strings | no | Restrict to a subset of images. Default: all. |

With this block present, `tailor export` and `tailor export --check` run with no arguments — the
`--check` form is a drift gate for a pre-commit hook or CI (it fails on any changed, missing, or stale
file). See [Export configs for a pipeline](../how-to/export-configs-for-a-pipeline.md).

## Image discovery

With no `images:` key, tailor discovers every `*/image.yaml` at depth 1 from the workspace root.

To curate explicitly:

```yaml
images:
  members:
    - "*"
    - tools
  exclude:
    - scratch
  inline:
    - name: tiny
      base:
        path: ./bases/tiny.img
      outputs:
        - format: cosi
      config:
        os:
          hostname: tiny
```

Relative paths in `tailor.yaml` resolve against the workspace root. Relative paths in an `image.yaml` resolve against that image directory.
