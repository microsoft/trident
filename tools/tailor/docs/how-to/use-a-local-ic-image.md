# Use a locally-built Image Customizer image

Sometimes the toolchain (or a tools-dir source) is an image you build **locally** and never push to
a registry — for example an Image Customizer image with extra host tools layered in, tagged
`my-ic:local`. tailor runs such images directly: no registry, no `localhost:5000`
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
it **without a registry pull**. Under `pull: never`, tailor requires the image to be present locally
(or a locked digest) and never contacts a registry — use it to guarantee an offline build.

## 2. How the image is identified (and whether it locks)

tailor inspects the local image and picks the most reproducible identifier available:

- If the image carries a **`RepoDigest`** (it was pulled from, or pushed to, a registry at some
  point), tailor runs it as `container@sha256:…` and that digest is **recorded in `tailor.lock`**.
- If the image is **local-only** (no `RepoDigest` — a fresh `docker build` that was never pushed),
  tailor runs it by its image **`Id`**. An `Id` is not portable across machines, so it is
  **intentionally omitted from `tailor.lock`**: a locked build cannot pin something another machine
  cannot fetch.

This applies identically to `toolsDirSources` and to an image's inline `toolchain:` / `toolsDir.source`.

## 3. Match the architecture

A local image is built for one architecture. tailor reads the local image's architecture during
resolution and **fails fast, before any container run,** if it cannot provide a selected cell's arch:

```
error: toolchain `local-ic` local image is `amd64` but cell `gizmo_arm64_cosi` targets `arm64`;
       no local image for that arch and pull policy won't fetch it
```

Build cells whose arch the local image can serve (for example select the matching arch, see
[Cross-arch building](cross-arch-building.md)), or provide a local image for the other arch.

## 4. Make a reproducible build

To pin a local image in the lockfile, give it a `RepoDigest` by pushing (or re-pulling) it through a
registry your builders can reach, then `tailor lock`. A purely local `Id`-based image stays usable
but is never locked — see [Pull policy](../reference/tailor-yaml.md#pull-policy).
