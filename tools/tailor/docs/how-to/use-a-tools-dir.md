# Use a tools dir for sealed images

Some sealed/minimal bases do not contain `tdnf`/`dnf`, so IC needs an external tools dir. Declare a
named source in `tailor.yaml`:

```yaml
toolsDirSources:
  - name: base
    container: mcr.microsoft.com/azurelinux/base/core
    tag: "3.0"
```

Then opt in from the image:

```yaml
toolsDir:
  source: base

config:
  previewFeatures:
    - tools-dir
```

tailor resolves the source digest and exports it once to a shared, digest-keyed cache under
`runtime.imageCacheDir/tools-dirs/<digest>`. For each cell it copies that cache to a per-cell
disposable directory `<buildDirBase>/<slug>/tools-dir`, binds **that copy writable**, and passes the
translated path to IC customize passes. It never emits `--tools-dir /`.

The tools dir is always writable because IC rewrites `resolv.conf` inside the tools chroot during
package operations — a read-only bind fails. `runtime.buildDirBase` defaults to
`<output>/.tailor/build`, so no configuration is required; set it to place the per-cell copy
elsewhere:

```yaml
# tailor.yaml
runtime:
  buildDirBase: /mnt/tailor-build
```
