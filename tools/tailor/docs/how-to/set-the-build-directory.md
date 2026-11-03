# Set the build directory

Use `runtime.buildDirBase` to put per-cell Image Customizer scratch on a chosen host filesystem.

## Set a workspace default

```yaml
# tailor.yaml
runtime:
  buildDirBase: /mnt/fast/tailor-build
```

Each selected cell gets its own child directory under that base. If you omit it, tailor uses
`<output-dir>/.tailor/build`.

## Override one run

```bash
tailor build --build-dir-base /mnt/fast/scratch
```

The flag overrides `runtime.buildDirBase` for that build. Use it in CI to point scratch at a
specific filesystem without changing the committed `tailor.yaml`.

## Safety guard

tailor refuses a build directory that is the filesystem root, a protected system directory such as
`/etc`, `/usr`, or `/var`, or `$HOME`. Use a dedicated scratch directory instead.

Related: [Use a tools dir for sealed images](use-a-tools-dir.md); see the
[`tailor.yaml` reference](../reference/tailor-yaml.md).
