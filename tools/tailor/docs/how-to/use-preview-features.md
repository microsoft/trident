# Enable a preview feature

Use `previewFeatures` in `tailor.yaml` before you use a feature that is not part of the stable
contract.

```yaml
schemaVersion: 1
previewFeatures: [signing]
```

Today, `signing` is the only accepted value. An unknown value is a hard error when tailor reads the
manifest.

A build or validate run that requests signing without this opt-in also fails. Preview features are
unstable: their schema and behavior may change until they are promoted.

Related: [Sign an image](sign-an-image.md); see the repository-root Compatibility document for the
stable contract.
