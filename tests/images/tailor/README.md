# Tailor test images

Each image family has one image.yaml containing its inputs, axes, base config
and conditional fragments. The legacy Make/Python definitions remain available
until their consumers are explicitly migrated.

## Layout

```text
tailor.yaml                     Job-local Image Customizer toolchain manifest
common/scripts.yaml             Named shared Bash bodies, selected with $include
common/files/                   Shared image-owned configuration/runtime assets
trident-vm-testimage/image.yaml  VM inputs, matrix and in-file variant deltas
trident-vm-testimage/.gitignore  Generated SSH inputs
```

The VM model remains named vm, with arch=amd64, scenario=base/update and
distro=azl3/azl4. Base produces QCOW2; update produces two distinct COSI clones.
AZL3 consumes the existing QEMU guest base; AZL4 consumes minimal-os:4.
The pipeline preserves legacy artifact names and the existing SSH-key producer.

## Sharing

In-file fragments apply in declaration order after the base. They use normal
Tailor match/merge rules; $set replaces inherited values/lists explicitly.
The tooling prerequisite supports selecting a node, for example:

```yaml
config:
  scripts:
    postCustomization:
      - interpreter: /bin/bash
        content:
          $include: ../common/scripts.yaml#/audit-directory
```

Includes are literal and image-directory-relative. Shared file sources use
../common/files. Library bodies preserve shell expressions through shellDollar;
lint the rendered Bash, not its interpolation template. image.yaml, the containing
library and common assets are declared dependencies for incremental invalidation.
Generated public keys remain model-local inputs, not committed common assets.

Image jobs reset canonical tailor.yaml from Git, select an existing dev container
only when requested, and archive the effective manifest in logs/tailor.yaml.
No alternate manifest is used. Future producers extend the existing library and
place new assets directly in common instead of moving them between image models.
