# Track local build inputs

Tailor hashes `rpmSources` and `extraDependencies` when deciding whether a cell needs rebuilding.
An IC config path alone is only a string: add files it references to `extraDependencies`.

```yaml
# appliance/image.yaml
name: appliance
base: { path: ../bases/input.img }
rpmSources:
  - ./rpms
  - ./repos/updates.repo
extraDependencies:
  - ./scripts/configure.sh
config:
  scripts:
    postCustomization:
      - path: scripts/configure.sh
```

Paths in `image.yaml` resolve from that image's directory. A changed file in `scripts` now
invalidates the cell's stamp; edits to RPM sources do likewise. Changes to merged IC config,
toolchain/base identity, output settings, and declared producer `inputs` also re-fingerprint the
cell. Directory hashes ignore RPM `repodata` for `rpmSources`.

Use `tailor build appliance --force` to rerun independently of the stamp. `--dry-run` previews
invocations without resolving a registry digest or writing a build stamp; `tailor validate` checks
the tailor configuration offline but does **not** run Image Customizer to validate its opaque
`config:` tree.

See [Image definition](../reference/image-yaml.md#interpolation-and-local-inputs).
