# tailor end-to-end fixture

A small, real workspace the **E2E workflow** (`.github/workflows/e2e.yml`) builds with the actual
`tailor` binary and Image Customizer — to prove the full product works, end to end. The images are
built but **not booted**.

It is intentionally a real (not synthetic) build, so it exercises the slow/privileged path the unit
and integration tests can't: pulling the IC image and an Azure Linux base from MCR and running a
privileged `customize` that produces real `.cosi` artifacts.

## What it covers

One image, `appliance`, built into two cells, touching a broad slice of tailor in a single build:

- an **`azureLinux` (MCR) base** — digest-pinned by tailor and downloaded by IC (this needs the
  `input-image-oci` preview feature, declared in the image's `config:`; tailor is config-opaque so
  the opt-in lives there);
- a **matrix axis** (`flavor: [min, net]`) → two cells / two artifacts;
- **per-axis fragments** (`by-flavor/`) that set a parameter and, for `net`, append a kernel
  command-line argument via list merge;
- **parameter interpolation** (`${hostname}`) into the IC config;
- **`$include`** to splice a shared storage layout into every cell;
- the built-in **defaults** for `runtime.imageCacheDir` and `runtime.janitorImage` (the workspace
  sets neither), and the **sudo-free janitor** (the workflow asserts the outputs are runner-owned,
  not root-owned).

The workflow also runs the pure verbs (`list`, `matrix`, `validate`, `explain`, `render`, `lock`),
which exercise the config/render layer without an engine.

## Run it locally

Requires Docker (the build runs a privileged IC container):

```bash
cd e2e
tailor build          # builds artifacts/appliance_{min,net}_amd64_cosi.cosi
```

## Extend it

- **Add a cell**: add a value to the `flavor` axis (and a `by-flavor/<value>.yaml` fragment), or add
  another matrix axis. Each cell is one real IC build, so keep the count modest to bound CI time.
- **Add an image**: drop in another `<name>/image.yaml`. The workflow's assertion loop lists the
  expected artifact slugs — add the new ones there.
- **Exercise more IC features**: extend the `config:` (e.g. packages, scripts, users). Package
  installs reach the network inside the build, so prefer them only when worth the extra time.
