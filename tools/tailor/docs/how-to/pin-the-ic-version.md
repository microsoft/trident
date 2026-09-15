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

## Freeze and refresh the lockfile

`tailor.lock` records the exact digest each toolchain (and base image) resolves to, so every machine
builds the same inputs.

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

`--locked` requires a complete lock and fails on any missing entry or registry drift, so a CI build
never silently resolves something new.

> **Local images.** A local-only Image Customizer image (a fresh `docker build` that was never pushed,
> so it has no registry digest) can't be pinned and is omitted from `tailor.lock`; reproducibility then
> depends on the local image tag you built. A local image that *does* carry a repo digest is still
> locked. See [Use a locally-built Image Customizer image](use-a-local-ic-image.md).
