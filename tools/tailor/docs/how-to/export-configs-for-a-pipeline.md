# Export configs for a pipeline

Sometimes a build pipeline cannot run tailor itself — for example an official, trust-sensitive
release pipeline that only runs reviewed, in-repo tooling. You can still author images with tailor's
matrix and merge model, then **export** the fully rendered Image Customizer config for each cell as a
committed file the pipeline consumes directly. tailor becomes a code generator; the pipeline runs
Image Customizer against the checked-in YAML with no tailor dependency.

## 1. Declare the export in `tailor.yaml`

```yaml
# tailor.yaml
export:
  outputDir: rendered      # committed directory, relative to the workspace root
```

`scope` defaults to `configsOnly` (one merged IC config per cell) and can be omitted. Optionally
restrict to a subset with `images: [<name>, …]`.

## 2. Generate the committed configs

```bash
tailor export
```

This writes one `rendered/<slug>.yaml` per cell and prunes any stale `*.yaml` a removed cell or axis
left behind. Commit the `rendered/` directory — reviewers see the exact configs the pipeline will
build, and the pipeline reads them without tailor.

## 3. Guard against drift in CI

Add a check so a config change can never be committed without regenerating:

```bash
tailor export --check
```

`--check` renders to a temporary directory and compares against the committed files, exiting non-zero
on any **changed**, **missing**, or **extra** (stale) file — without writing anything. Wire it into a
pre-commit hook and a PR CI job:

```yaml
# PR pipeline step
- run: tailor export --check
```

Because the export is declared in `tailor.yaml`, both commands are argument-free, so the CI step is a
one-liner.

## 4. What the pipeline supplies

The exported `<slug>.yaml` is only the Image Customizer **config**. Everything else in an IC
invocation — the base image, `--rpm-source`, `--tools-dir`, output format, and any signing — is not in
the config and is provided by the pipeline. This is deliberate: the config is portable and static, so
`tailor export` renders any cell whose config is complete, and the pipeline owns the invocation using
its own approved machinery.

A minimal pipeline call per cell looks like:

```bash
imagecustomizer \
  --config-file rendered/<slug>.yaml \
  --image-file <base-for-that-cell> \
  --output-image-file out/<slug>.<ext> \
  --output-image-format <format>
```

Pin the Image Customizer image the pipeline runs to the digest in your committed `tailor.lock` so it
matches what tailor resolved.
