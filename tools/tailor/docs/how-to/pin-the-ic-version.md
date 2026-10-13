# Pin the Image Customizer version

Pin Image Customizer in `tailor.yaml` under `toolchains:`.

```yaml
schemaVersion: 1

toolchains:
  default: ic-1.3
  entries:
    - name: ic-1.3
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      version: "1.3.0"
```

`tag` defaults to `version`, or to `latest` when neither `tag` nor `version` is set. Use `tag` if the registry tag is not the version string:

```yaml
toolchains:
  default: nightly
  entries:
    - name: nightly
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      tag: latest
```

An image can select a non-default toolchain:

```yaml
# db/image.yaml
name: db
toolchain: ic-1.3
```

## Freeze and refresh the lockfile *(preview)*

> **Preview feature.** `tailor lock`, `tailor update`, and `tailor build --locked` are gated behind
> the `lock-preview` Cargo feature and are **absent from default builds** (lockfile enforcement is not
> yet implemented). Install with `cargo install --features lock-preview` to use the commands below.

`tailor.lock` can record named toolchains, tools-dir sources, registry bases, and an explicitly
configured janitor image. Builds can use existing pins; ordinary builds do not write the lock.
The version and tag settings above are available without the preview feature.

```bash
# First time — write supported pins:
tailor lock

# Add a new image/toolchain later — lock keeps existing pins:
tailor lock

# Deliberately refresh pins according to each source's pull policy:
tailor update
```

`lock` is a **freeze**: inputs already pinned keep their digests, so re-running it is idempotent and
never silently moves a pin. It **still resolves registry bases** before reusing their pins, so
neither `lock` nor a build using a registry base is necessarily offline. `update` ignores prior
pins and resolves inputs under their pull policies; locally selected images need not change.
The built-in janitor and inline image sources are not added to the lock.

## Lockfile enforcement is not available yet

**Do not rely on `--locked` as a CI drift gate.** The flag exists only in `lock-preview` builds,
and strict lockfile enforcement is not implemented. Verify resolved inputs separately if your
pipeline requires a complete, immutable lock.

> **Local images.** A local-only Image Customizer image (a fresh `docker build` that was never pushed,
> so it has no registry digest) can't be pinned and is omitted from `tailor.lock`; reproducibility then
> depends on the local image tag you built. A local image that *does* carry a repo digest can be
> pinned when it is a named workspace source and you run the preview `lock` command.
> See [Use a locally-built Image Customizer image](use-a-local-ic-image.md).
