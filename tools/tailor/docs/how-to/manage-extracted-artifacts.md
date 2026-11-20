# Manage extracted Image Customizer artifacts

When IC extracts `output.artifacts`, choose what tailor does with the extraction on **unsigned**
cells. Enable IC's `output-artifacts` feature in the image's `config:`:

```yaml
# image.yaml
name: appliance
outputArtifacts: managed
base: { path: ./input.img }
config:
  previewFeatures: [output-artifacts]
  output:
    artifacts:
      items: [ukis]
      path: ./extracted
```

The default `managed` policy replaces `path` in the execution copy of the config and publishes
extracted files at `artifacts/<slug>.artifacts`, owned by your user. The authored config and
`tailor render` output are unchanged. Choose `scratch` to reclaim the extraction after the run,
or `strip` to omit extraction entirely:

```yaml
# tailor.yaml (optional default for unsigned images)
defaults:
  outputArtifacts: scratch
```

An image's `outputArtifacts` overrides this default. Without `config.previewFeatures:
[output-artifacts]`, tailor does not rewrite the IC config. Signing manages extraction
independently; see [Sign an image](sign-an-image.md).

See [Image definition](../reference/image-yaml.md#staging-extracted-artifacts).
