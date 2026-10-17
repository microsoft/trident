# Select an Image Customizer tag

Choose the workspace's default Image Customizer container in `tailor.yaml`:

```yaml
schemaVersion: 1
toolchains:
  default: ic
  entries:
    - name: ic
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      version: "1.3.0"
      pull: missing
```

`version` is SemVer metadata and supplies the registry tag if `tag` is absent. An explicit
`tag` takes precedence; with neither field, the tag is `latest`. Images may choose another named
toolchain with `toolchain: <name>`. `pull: missing` (default) normally uses a local image, then
resolves/pulls from the registry if absent; `always` requests a pull, while `never` normally
requires a local image. The policy applies to this container source, not separately declared
registry base images.

Tags, **including version-shaped tags**, can move. Selecting `version: "1.3.0"` or
`tag: "1.3.0"` does not guarantee identical bytes on later machines or runs. To inspect the
current resolution, run `tailor resolve` with a reachable engine (it may contact registries):

```bash
tailor resolve
```

It prints a YAML summary of resolved inputs to stdout without building or writing files.
Results from a later run may differ if a tag changes. See
[Pull policy](../reference/tailor-yaml.md#pull-policy) and
the [CLI reference](../reference/cli.md#tailor-resolve-images).
