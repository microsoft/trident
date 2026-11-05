# Build dependent images

Use a workspace when one image consumes another's artifact. Define both images as members; tailor
builds producer cells before their consumers, including when you name only the consumer.

For a producer with a single `raw` output:

```yaml
# base-os/image.yaml
name: base-os
base: { path: ../bases/input.img }
matrix:
  arch: [amd64, arm64]
outputs:
  - format: raw
config:
  os:
    hostname: base-os
```

Select its output as another image's base:

```yaml
# appliance/image.yaml
name: appliance
base: { image: base-os, output: raw }
matrix:
  arch: [amd64, arm64]
config:
  os:
    hostname: appliance
```

```bash
tailor validate appliance
tailor build appliance --dry-run
tailor build appliance --arch amd64
```

The shared `arch` pairs each appliance cell with its producer cell. If the producer has multiple
outputs, specify `output` by **format** (not file extension). For producer-only axes, pin a
coordinate with `cell: { flavor: minimal }` on the `base:` reference. Missing or ambiguous pairs
fail validation; dependency cycles fail before building.

To refer to an artifact *inside* the IC config instead of using it as the base, declare an input:

```yaml
inputs:
  - name: payload
    image: base-os
    output: raw
config:
  os:
    additionalFiles:
      - source: "${inputs.payload}"
        destination: /opt/payload.raw
```

Tailor substitutes the paired artifact path, hashes its contents for incremental rebuilds, and
builds its producer first. Use `dependsOn: [base-os]` only when you need build ordering **without**
consuming its bytes; order-only edges do not change the consumer's fingerprint.

See [Inter-image dependencies](../reference/image-yaml.md#inter-image-dependencies).
