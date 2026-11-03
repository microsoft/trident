# Change an output format

`outputs:` is a list. Lists append by default, so use `$replace` when a fragment should swap the inherited outputs instead of adding another artifact.

```yaml
# image.yaml
outputs:
  - format: cosi
```

```yaml
# by-edition/pro.yaml
outputs:
  $replace:
    - format: raw
```

To add an extra output instead, use normal list syntax:

```yaml
# by-edition/debug.yaml
outputs:
  - format: qcow2
```

A build produces one artifact per selected cell × output. Valid formats are listed in [output formats](../reference/output-formats.md).
