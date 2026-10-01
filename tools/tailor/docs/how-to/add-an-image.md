# Add an image to a workspace

Use this when you already have a `tailor.yaml` workspace.

```bash
tailor add image web
```

This requires a `tailor.yaml` in the current directory or a parent directory. It creates `web/image.yaml` in the current directory and registers it in the workspace manifest.

Then list images:

```bash
tailor list
```

Expected shape:

```text
Images:
  web                          1 cell(s)
```

Edit `web/image.yaml` to set `base:`, `outputs:`, and the opaque Image Customizer `config:` tree.

See also: [image.yaml reference](../reference/image-yaml.md) and [tailor.yaml reference](../reference/tailor-yaml.md).
