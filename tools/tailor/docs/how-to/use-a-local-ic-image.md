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
it **without a registry pull**. Under `pull: never`, tailor requires the image to be present locally
when no digest is locked and fails if it is missing. An existing lock digest takes precedence
and may trigger a pull even with `pull: never`; registry **base** resolution is independent. This
policy alone does not guarantee an offline build.

## 2. How the image is identified (and whether it locks)

tailor inspects the local image and picks the most reproducible identifier available:

- If the image carries a **`RepoDigest`** (it was pulled from, or pushed to, a registry at some
  point), tailor runs it as `container@sha256:…`. A feature-enabled `tailor lock` can record
  named workspace toolchain/tools-dir source digests; an ordinary build does **not** write the lock.
- If the image is **local-only** (no `RepoDigest` — a fresh `docker build` that was never pushed),
  tailor runs it by its image **`Id`**. An `Id` is not portable across machines, so it is
  **not lockable**: another machine cannot fetch that local image by its ID.

Inline `toolchain:` / `toolsDir.source` images can run locally by digest or ID, but `tailor lock`
does not add standalone inline sources to its named workspace lists.

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

To record a **named** source in the lockfile, give it a `RepoDigest` by pushing (or re-pulling) it
through a registry your builders can reach, then use a `lock-preview` build's `tailor lock`.
Strict `build --locked` enforcement is not implemented; a purely local `Id` remains usable but
unlocked. See [Pull policy](../reference/tailor-yaml.md#pull-policy).
