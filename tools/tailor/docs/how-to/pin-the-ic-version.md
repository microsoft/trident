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

`tailor.lock` records digests for resolvable toolchains and registry bases; builds can use existing
pins. The version and tag settings above are available without the preview feature.

```bash
# First time — resolve everything and write the lock:
tailor lock

# Add a new image/toolchain later — lock keeps existing pins and resolves only the new inputs:
tailor lock

# Deliberately move every pin to the latest digest:
tailor update
```

`lock` is a **freeze**: inputs already pinned keep their digests, so re-running it is idempotent and
never silently moves a pin. `update` re-resolves **everything** to the latest digest — use it when you
intend to move up.

## Build reproducibly from the lock

```bash
tailor build --locked
```

**Do not rely on `--locked` as a CI drift gate yet.** The flag exists only in `lock-preview`
builds, and strict lockfile enforcement is not implemented. For now, verify resolved inputs by
other means if your pipeline requires a complete, immutable lock.

> **Local images.** A local-only Image Customizer image (a fresh `docker build` that was never pushed,
> so it has no registry digest) can't be pinned and is omitted from `tailor.lock`; reproducibility then
> depends on the local image tag you built. A local image that *does* carry a repo digest is still
> locked. See [Use a locally-built Image Customizer image](use-a-local-ic-image.md).
