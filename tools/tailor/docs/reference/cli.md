# CLI reference

Global options:

| Option | Meaning |
| --- | --- |
| `--manifest <PATH>` | Path to `tailor.yaml`, or a directory to search from. Default: walk up from the current directory. |
| `--engine <docker\|podman\|auto>` | Container engine for this invocation. Overrides `runtime.engine`. See [Select a container engine](../how-to/select-a-container-engine.md). |
| `--host <ENDPOINT>` | Engine endpoint for this invocation (`unix://…` or `tcp://…`). Overrides `runtime.host` and `DOCKER_HOST` / `CONTAINER_HOST`. |
| `--log-dir <PATH>` | Persist each cell's full IC debug log to `<PATH>/<slug>.log`. |
| `--ic-log-level <panic\|fatal\|error\|warn\|info\|debug\|trace>` | Set IC's own log level, independent of `-v`/`-q`. |
| `--strict` | Promote authority/confinement warnings to errors. |
| `-v`, `--verbose` | Increase verbosity; repeatable. |
| `-q`, `--quiet` | Decrease verbosity; repeatable. |
| `--timestamps <elapsed\|time\|off>` | Leading timestamp on status/log lines (default elapsed). |
| `--version` | Print version, including commit/build metadata. |

## `tailor init <name> [base|simple|advanced]`

Scaffold a project. Omitted template is `base`.

| Template | Creates |
| --- | --- |
| `base` | `tailor.yaml` plus `<name>/image.yaml`. |
| `simple` | Standalone `./image.yaml`; no `tailor.yaml`; uses built-in default IC toolchain. |
| `advanced` | Like `base`, plus `variant` and `arch` axes, `by-variant/`, `by-arch/`, and `${efiArch}` interpolation. |

## `tailor add image <name>`

Add a member image to an existing workspace. Requires a `tailor.yaml` in the current directory or a parent. Creates `<name>/image.yaml` in the current directory and registers it in `tailor.yaml`.

## `tailor add axis [<image>] <axis>`

Append an axis to an image's `matrix:` and create `by-<axis>/`. The image argument is optional when the workspace has a single image. A placeholder value is inserted so the matrix stays non-empty.

## `tailor build [images...]`

Resolve and run Image Customizer for selected images. Default: all images.

Each positional may be an **image name** or a **cell slug**. A slug (e.g.
`gizmo_pro_arm64_stable_cosi`) builds exactly that cell of its owning image, so you don't have to
name the image and pass `--cell` — `tailor build <slug>` is shorthand for
`tailor build <image> --cell <slug>`. Run `tailor slugs` to list cells.

| Flag | Meaning |
| --- | --- |
| `-s`, `--select AXIS=VALUE` | Constrain matrix axes. Repeatable. Comma-separated axis pairs are accepted, for example `-s variant=full,arch=amd64`. |
| `--cell SLUG` | Select exact cells by slug. Repeatable. |
| `--locked` | Require a complete `tailor.lock`; fail on missing entries or registry drift. |
| `--force` | Ignore incremental up-to-date checks. |
| `--arch ARCH` | Restrict build to architecture(s). Repeatable. |
| `--output-dir PATH` | Output directory. Default: `<workspace>/artifacts`. |
| `--build-dir-base PATH` | Override `runtime.buildDirBase`: place each cell's build scratch under this directory (which must not be `/`, a system directory, or `$HOME`). Lets CI point scratch at a specific filesystem without editing the committed `tailor.yaml`. |
| `--dry-run` | Render each selected container/IC invocation without running it. |
| `--clones N` | Build N clones of each cell — distinct artifacts sharing all meaningful content but differing in incidental details (fresh UUIDs, timestamps), each published as `<slug>_clone<n>`. Default: `1`. |

## `tailor convert <input> --to <format>`

Convert a single image file to another format via Image Customizer (`convert`) — no workspace or
config required. Writes the output beside the input by default (or to `-o`), owned by your user.

| Flag | Meaning |
| --- | --- |
| `--to FORMAT` | Target format (required): `vhd`, `vhd-fixed`, `vhdx`, `qcow2`, `raw`, `cosi`, `baremetal-image`. |
| `-o`, `--output PATH` | Output path. Default: the input's name with the target extension, beside the input. |
| `--container REF` | The Image Customizer image to run. Default: `mcr.microsoft.com/azurelinux/imagecustomizer:latest`. |
| `--arch ARCH` | `amd64` (default) or `arm64` — drives `--platform linux/<arch>`. |
| `--build-dir-base PATH` | Host base for IC scratch. Default: a unique dir under the system temp dir. Must not be `/`. |
| `--dry-run` | Render the container invocation without running it. |

See [Convert an image format](../how-to/convert-an-image-format.md).

## `tailor validate [images...]`

Render every selected cell without building. Catches tailor-owned config and merge errors. Accepts `-s/--select` and `--cell`.

## `tailor matrix [images...] [--format json|slugs|ado]`

Emit selected matrix cells. Default format is `json`.

JSON entries contain `image`, `slug`, `axes`, and `format`, plus `baseImage` when the cell binds to a
`baseImages:` catalogue slot.

| Flag | Meaning |
| --- | --- |
| `--format json` | JSON array of cell objects (default). |
| `--format slugs` | One cell slug per line — feeds `tailor build --cell <slug>` directly. |
| `--format ado` | The bare Azure DevOps matrix object (`{ leg: { var: string, … } }`) for a pipeline `strategy.matrix`. |
| `--ado VAR_NAME` | Emit the ADO matrix wrapped in a `##vso[task.setvariable]` logging command that sets `VAR_NAME` (e.g. `BUILD_MATRIX`). Implies `--format ado` and conflicts with `--format`. An empty selection exits non-zero. |

## `tailor slugs [images...]`

Print one selected cell slug per line. Equivalent to `tailor matrix --format slugs`.

## `tailor explain <image>`

Print the **merge order** for each selected cell: the ordered list of fragment files that merge into it
(base first, later files win), each annotated with why it applies and any `$include`d libraries. This makes
the fragment precedence model legible. Add `--with-config` to also print the merged Image Customizer
config. Accepts `-s/--select` and `--cell`; read-only and offline.

```text
$ tailor explain gizmo --cell gizmo_pro_arm64_stable_cosi
cell  gizmo_pro_arm64_stable_cosi   (arch=arm64, channel=stable, edition=pro)

merge order (top = base, bottom wins):
   1  image.yaml                      base
   2  by-edition/pro.yaml             edition=pro
   3  by-arch/arm64.yaml              arch=arm64
   4  by-channel/stable.yaml          channel=stable
   5  by-edition+arch/pro+arm64.yaml  edition=pro ∧ arch=arm64
```

## `tailor show <image> [field]`

Show dimensions and cell count for one image. Optional fields currently include `name`, `dir`, `outputs`, and `features`.

## `tailor list`

List images and toolchains.

## `tailor render [images...]`

Write golden snapshots for selected cells. Accepts `-s/--select` and `--cell`.

## `tailor export [images...]`

Render each selected cell's merged Image Customizer config to a committed directory (one
`<slug>.yaml` per cell), so a pipeline can build the images **without tailor**. Offline and pure —
no base/toolchain resolution, no Docker.

Configure it once in `tailor.yaml` so the command is argument-free (ideal for a pre-commit hook or CI
gate):

```yaml
export:
  outputDir: rendered      # committed output dir (relative to the workspace root)
  # scope: configsOnly     # optional; defaults to configsOnly (the only scope today)
  # images: [gadget]       # optional; default = all images
```

| Flag | Meaning |
| --- | --- |
| `--check` | Verify the committed exports match freshly rendered configs; exit non-zero on any changed, missing, or extra (stale) file. Writes nothing. |
| `--output-dir DIR` | Output directory. Default: `export.outputDir` from `tailor.yaml`. |
| `-s`, `--select AXIS=VALUE` | Constrain matrix axes. Repeatable. |
| `--cell SLUG` | Select exact cells by slug. Repeatable. |

`tailor export` writes each `<slug>.yaml` and prunes any stale `*.yaml` in the output directory that no
selected cell produces. Only static cells are exportable — the config YAML itself carries no
tools-dir, base, rpm-source, or signing details (those are Image Customizer invocation arguments the
consuming pipeline supplies).

## `tailor lock`

Resolve registry inputs and write `tailor.lock` without building. Inputs already pinned in the
current lock **keep their digests** — only new or unpinned inputs are resolved, so re-running `lock`
is idempotent and never silently moves an existing pin. Use it to freeze a reproducible set.

## `tailor update`

Re-resolve **every** input to its latest digest and rewrite `tailor.lock`, ignoring the existing
pins. Use it to deliberately refresh to newer base images / toolchains.

## `tailor resolve [images...]`

Resolve digests/hashes and print the lockfile content without writing it.

## `tailor clean [images...]`

Remove generated artifacts and build stamps for selected cells. Accepts `-s/--select` and `--cell`.

## `tailor bases list`

List every base-image catalogue slot with its arch, `source` (if any), on-disk presence, and path.
Read-only; requires a `baseImages:` catalogue in `tailor.yaml`.

## `tailor bases download [names...] [--force]`

Materialise base-image catalogue slots from their `source`. Default (no names): every slot that has a
`source` and whose file is missing. Naming a sourceless slot is an error; `--force` re-pulls present
files. Requires a `baseImages:` catalogue in `tailor.yaml`.

## `tailor bases verify [names...]`

Assert base-image slot files exist on disk, failing with the missing names and paths. Default scope is
every slot referenced by the workspace's images; pass names to check only those. The pipeline's "is the
feed download wired?" gate. See [Use a base-image catalogue](../how-to/use-a-base-image-catalogue.md).

## `tailor version`

Print version information. Same source as `tailor --version`.

## `tailor notice`

Print tailor's own MIT license, then the third-party software notices for every dependency compiled
into the binary — each crate's name, version, SPDX identifier, and full license text. The notice is
generated at build time from the resolved dependency set (`Cargo.lock`), so it always matches the
binary you are running. It writes to stdout only (it never creates files); redirect it to archive the
attributions:

```bash
tailor notice > THIRD-PARTY-NOTICES.txt
```

## Exit codes

Every command uses a small, stable exit-code taxonomy so scripts and CI can branch on the outcome:

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | Build or operational failure (an Image Customizer run failed, an engine error, an I/O error). |
| `2` | Usage or configuration error — bad arguments, an unknown image, an invalid `tailor.yaml`/`image.yaml`, or a dependency cycle. Mirrors clap's own code for bad arguments. |
| `130` | Interrupted (Ctrl+C / SIGTERM): `128 + SIGINT`. The running container is torn down before exit. |

See [Handle exit codes in scripts](../how-to/handle-exit-codes.md).
