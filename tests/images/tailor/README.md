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


## Host test-image model

trident-testimage/image.yaml contains the AMD64/base/AZL4 host recipe and COSI
output. It reuses the existing audit-directory and AZL4 script-library entries,
common assets, staged AZL4 base and RPMs. Its source dependencies include the
model, containing library and common files.

The native host job publishes two clones under the existing
trident-azl4-testimage artifact name. It does not stage VM SSH keys: the installer
and Host Configuration create the test user. Production remains separate from
test selection/adoption; adding this model does not change consumer defaults.
