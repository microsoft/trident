# Your first matrix

This tutorial shows how a single image definition expands into multiple cells.

Unlike [Getting started](getting-started.md), which built one standalone `image.yaml`, this tutorial
uses a **workspace**: a `tailor.yaml` manifest (toolchains, runtime defaults, and which images belong
to the workspace) alongside one or more member images, each in its own directory. `schemaVersion: 1`
is required in `tailor.yaml`.

A few terms used throughout:

- **matrix** — the axes you declare on an image; their cartesian product is the set of candidate builds.
- **axis** — one dimension of the matrix (e.g. `variant`, `arch`), each with a list of values.
- **cell** — one combination of axis values: a single concrete build.
- **slug** — a cell's stable identifier (`<image>_<axis values>_<format>`), also the artifact name.
- **fragment** — a `by-<axis>/<value>.yaml` file whose config is merged in only for cells with that
  value.

See [Concepts](../explanation/concepts.md) for the full model.

## 1. Scaffold an advanced workspace

```bash
mkdir matrix-demo
cd matrix-demo
tailor init gizmo advanced
```

The `advanced` template creates:

```text
tailor.yaml
gizmo/image.yaml
gizmo/by-variant/minimal.yaml
gizmo/by-variant/full.yaml
gizmo/by-arch/amd64.yaml
gizmo/by-arch/arm64.yaml
```

## 2. Inspect the matrix

```bash
tailor matrix gizmo --format slugs
```

Expected output:

```text
gizmo_minimal_amd64_cosi
gizmo_minimal_arm64_cosi
gizmo_full_amd64_cosi
gizmo_full_arm64_cosi
```

The slug format is `<image>_<axis values in matrix order>_<format>`.

## 3. Add another axis

```bash
tailor add axis gizmo channel
```

This appends a placeholder value to `gizmo/image.yaml` and creates `gizmo/by-channel/`. Edit the new matrix entry:

```yaml
matrix:
  variant: [minimal, full]
  arch:    [amd64, arm64]
  channel: [stable, edge]
```

Add channel fragments:

```bash
cat > gizmo/by-channel/stable.yaml <<'EOF'
params:
  repoChannel: stable
config:
  os:
    packages:
      install:
        - gizmo-stable
EOF

cat > gizmo/by-channel/edge.yaml <<'EOF'
params:
  repoChannel: edge
config:
  os:
    packages:
      install:
        - gizmo-edge
EOF
```

## 4. Watch cells multiply

```bash
tailor matrix gizmo --format slugs
```

Expected shape: eight slugs, because `variant[2] × arch[2] × channel[2] × outputs[1] = 8`.

```text
gizmo_minimal_amd64_stable_cosi
gizmo_minimal_amd64_edge_cosi
...
gizmo_full_arm64_edge_cosi
```

## 5. Inspect rendered Image Customizer YAML

Narrow to a single cell with `--select` (short form `-s`), an `axis=value` filter you can repeat or
comma-separate:

```bash
tailor explain gizmo -s variant=full,arch=amd64,channel=edge --with-config
```

Expected shape:

```text
gizmo: 1 cell(s)

── gizmo_full_amd64_edge_cosi ──
os:
  hostname: gizmo
  packages:
    install:
      - openssh-server
      - grub2-efi-x64
      - vim
      - git
      - gizmo-edge
```

The exact config depends on your edits. The important point: `explain --with-config` shows the fully merged IC config for selected cells (plain `explain` shows only the ordered list of fragment files that merge into each cell).

## 6. Dry-run one selected cell

```bash
tailor build gizmo -s variant=full,arch=amd64,channel=edge --dry-run
```

You now have a small workspace that demonstrates axes, fragments, interpolation, cell selection, and dry-run builds.

## Next steps

- [Select and build one cell](../how-to/select-and-build-one-cell.md) — narrow a matrix with selectors and slugs.
- [Add an axis](../how-to/add-an-axis.md) — grow the matrix with a new dimension.
- [Merge model](../explanation/merge-model.md) — how fragments combine and which one wins.
- [CLI reference](../reference/cli.md) and [image.yaml reference](../reference/image-yaml.md) — look up every command and field.
