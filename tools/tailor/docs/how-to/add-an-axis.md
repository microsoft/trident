# Add an axis to an image

Use an axis when one image should build several variants.

```bash
tailor add axis web channel
```

If the workspace has exactly one image, the image argument is optional:

```bash
tailor add axis channel
```

The command appends a placeholder axis value so the matrix remains valid and creates the fragment directory:

```text
web/by-channel/
```

Edit `web/image.yaml`:

```yaml
matrix:
  channel: [stable, edge]
```

Create fragments:

```bash
cat > web/by-channel/edge.yaml <<'EOF'
config:
  os:
    packages:
      install:
        - web-edge-tools
EOF
```

Fragments apply in `matrix:` axis declaration order. If two later fragments deliberately override a scalar, use `$set`.
