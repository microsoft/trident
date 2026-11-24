# Use a locally-built Image Customizer image

Sometimes the toolchain (or a tools-dir source) is an image you build **locally** and never push to
a registry — for example an Image Customizer image with extra host tools layered in, tagged
`my-ic:local`. tailor runs such images directly: no registry, no local registry
workaround.

## 1. Reference the local image

Name the local image in `toolchains:` (or `toolsDirSources:`) exactly as it is tagged locally, and
leave `pull` at its default (`missing`) or set it to `never`:

```yaml
# tailor.yaml
schemaVersion: 1

toolchains:
  default: local-ic
  entries:
    - name: local-ic
      container: my-ic      # a local `docker images` tag, not a registry path
      tag: local
      pull: missing                        # missing (default) uses the local image when present
```

Build as usual:

```bash
tailor build
```

Under `pull: missing`, tailor inspects the local daemon, finds `my-ic:local`, and runs
it **without a registry pull** in the normal local-image flow. `pull: never` normally requires
a local image instead of fetching the toolchain. Registry **bases** resolve independently, so
this setting alone does not make the entire build offline.

## 2. How the image is identified

tailor inspects the local image and chooses an identifier for this run:

- If the image carries a **`RepoDigest`** (it was pulled from, or pushed to, a registry at some
  point), tailor runs it as `container@sha256:…`.
- If the image is **local-only** (no `RepoDigest` — a fresh `docker build` that was never pushed),
  tailor runs it by its image **`Id`**. That identifier is not portable across machines.

This also applies to inline `toolchain:` and `toolsDir.source` images.

## 3. Match the architecture

A local image is built for one architecture. tailor reads the local image's architecture during
resolution and **fails fast, before any container run,** if it cannot provide a selected cell's arch:

```
error: toolchain `local-ic` local image is `amd64` but cell `gizmo_arm64_cosi` targets `arm64`;
       no local image for that arch and pull policy won't fetch it
```

Build cells whose arch the local image can serve (for example select the matching arch, see
[Cross-arch building](cross-arch-building.md)), or provide a local image for the other arch.

## 4. Use the image on another machine

Build or distribute the same toolchain image on each builder, or push it to a registry they can
access. A shared tag alone does not guarantee identical bytes across machines. See
[Pull policy](../reference/tailor-yaml.md#pull-policy).
