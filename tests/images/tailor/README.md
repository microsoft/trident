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


## Installer and MOS models

trident-installer/image.yaml declares runtime=host/container and distro=azl3/azl4.
trident-mos/image.yaml declares distro=azl3/azl4. All six AMD64/base cells produce
ISOs; packages, services, storage and script differences stay in those manifests.
Script bodies extend common/scripts.yaml; live console/config assets live in
common/files. Distinct root profiles and services keep role-specific names.

Generated MOS policy, systemd, PCR and RCP inputs remain in ignored staging paths,
not source copies. Model/library/common files are declared cache dependencies.
AZL3 keeps its containerd policy module; AZL4 keeps explicit Ethernet DHCP.
Producer artifacts remain separate from installer selection and adoption.

The adoption layer removes only the legacy standard host/container installer
producers and supplies their stage/artifact names to the native templates.
Existing AZL3 consumers therefore retain their contracts. AZL4 host tests opt
into the native AZL4 installer through `useDistroInstaller`; the shared test
templates keep their prior installer choice by default. MOS artifacts are
exposed without the comparison prefix for download/local netlaunch use.
Split/direct-streaming installers, ARM64 images and azl-installer remain on
their existing build paths.


## Root-verity producer

trident-verity-testimage/image.yaml starts the root/usr family at its final path.
Current cells are AMD64/base, runtime=host/container, verity=root, distro=azl3/azl4.
The pipeline builds two COSI clones under comparison-prefixed tailor- artifacts;
existing legacy verity producers and consumer choices are unchanged.

Image-owned overlay assets use common/files and shell bodies extend the existing
common/scripts.yaml. The verity etc-mount script remains distinct from the VM
machine-ID-aware version. /usr-verity and its UKI signing/output contract will
extend this same manifest in a separate producer/adoption step.
AZL3 retains dracut-overlayfs; AZL4's dracut package owns its overlayfs and
systemd-veritysetup modules, with no separate overlayfs subpackage.
The AZL4 host installs selinux-policy so bootloader reset can read its config;
the image profile still disables SELinux.
