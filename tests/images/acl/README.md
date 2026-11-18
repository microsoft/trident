# ACL-T image prototype

`acl` and `acl-mshv` are ordinary `testimages.py` QCOW2 targets. Image
Customizer installs the candidate Trident RPMs and regenerates verity and the
UKI directly from the real ACL-T template. There is no separate image builder.
The existing synthetic fixtures and their pipeline remain unchanged.

## Inputs

The base is pinned to `aclmain` run **1260474**, artifact
`drop_build_acl_template_amd64_customize_and_sign`, file
`acl_template_image.vhdx`. This is the ACL-T source selected by
`acl-baremetal-devel` run **1261060**, not a generic Azure Linux image.
The download verifies the VHDX SHA256 before publishing it under `artifacts/`.

Use the ACL-enabled Image Customizer revision from that consumer:
`microsoft/azure-linux-image-tools@c8dc566c0c04d33e2deaa6a484113e0545997f92`,
layered onto the official IC 1.7 container. For example, the producer's
`acl-baremetal/tools/ic/build.py --ref <sha> --tag acl-ic:local`
builds this existing toolchain. Stock IC does not support the `acl.*` options.

Stage matching **AZL3** candidate RPMs in `bin/RPMS`, including `trident`,
`trident-acl`, `trident-selinux` and `trident-static-pcrlock-files`. Public
repositories exclude `trident*`, and embedded base-image repos are disabled,
so they cannot silently supply a different candidate. Do not mix AZL4 RPMs
into this directory.

For MSHV, also stage the efitreeloader RPM from the producer's
`drop_build_efitreeloader_rpm` artifact in `artifacts/rpm-overrides`.
Run 1261060 supplies a build from efitreeloader revision `bf02d4e3f4`.
The remaining MSHV packages come from the Azure Linux repositories.

## Build

Authenticate `az devops` for `https://dev.azure.com/mariner-org` first.
The first build downloads the pinned base; it can also be fetched explicitly
with `python3 tests/images/testimages.py download-image acl-t`.

```bash
python3 tests/images/testimages.py build acl --container acl-ic:local --output-dir artifacts/acl
python3 tests/images/testimages.py build acl-mshv --container acl-ic:local --output-dir artifacts/acl-mshv
```

Both builds generate SSH keys in their output directory. These unsigned test
images enable public-key SSH for `testuser` (with passwordless sudo) and DHCP,
select the `metal` OEM, and disable the automatic ACL agent. The existing ACL
root-SSH denial remains intact. Private keys are named
`key_acl_private.pem` and `key_acl-mshv_private.pem`.
ACL's Btrfs, A/B partition identities, partition sizes and overlay
layout are retained. The MSHV recipe additionally installs its loader chain;
merely replacing the Linux kernel would not boot the Microsoft hypervisor.

These are bootable-image prototypes, not replacement servicing fixtures yet.
The selected IC still reports ACL COSI bootloaders as `systemd-boot`, including
MSHV images, so COSI servicing integration needs separate validation.
