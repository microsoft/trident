# tailor

<p align="center">
  <img src="resources/logo_small.png" alt="tailor" width="360">
</p>

**Manifest-driven front-end for the Azure Linux Image Customizer.**

tailor lets you describe Azure Linux images in small YAML definitions instead of hand-writing Docker/Image Customizer invocations. It merges layered `image.yaml` fragments, expands matrices into build cells, resolves base images, and runs the Azure Linux Image Customizer (`mcr.microsoft.com/azurelinux/imagecustomizer`) once per cell. The `config:` tree remains Image Customizer YAML: tailor passes it through without modeling the IC schema.

```mermaid
flowchart LR
  A["image.yaml"]
  F["by-&lt;axis&gt;/&lt;value&gt;.yaml fragments"]
  A --> M["matrix expansion"]
  F --> M
  M --> C["cells"]
  C --> IC["Image Customizer container"]
  IC --> O["artifacts"]
```

## Documentation

This documentation is organized into four sections, each with a different job.

```mermaid
flowchart TD
  D["tailor docs"] --> T["Tutorials: learn by doing"]
  D --> H["How-to guides: solve a task"]
  D --> R["Reference: look up facts"]
  D --> E["Explanation: understand why"]
```

## Sections

- [Tutorials](tutorials/README.md) — start here if you are new to tailor.
- [How-to guides](how-to/README.md) — focused recipes for common tasks.
- [Reference](reference/README.md) — complete CLI and YAML details.
- [Explanation](explanation/README.md) — the model behind workspaces, matrices, cells, and merging.

## Quick links

- [Installation](installation.md)
- [Getting started](tutorials/getting-started.md)
- [Your first matrix](tutorials/your-first-matrix.md)
- [CLI reference](reference/cli.md)
- [Image definition reference](reference/image-yaml.md)
- [Merge directives](reference/directives.md)
- [Core concepts](explanation/concepts.md)

## Project

- [Compatibility policy](https://github.com/frhuelsz/tailor/blob/main/COMPATIBILITY.md) — what is
  stable across releases.
- [Changelog](https://github.com/frhuelsz/tailor/blob/main/CHANGELOG.md) — release history.
- [Releases](https://github.com/frhuelsz/tailor/releases) — signed binaries and provenance; see
  [Installation](installation.md) for verification.
