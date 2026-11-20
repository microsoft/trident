# Native ACL images

The ordinary `testimages.py` targets use two static Image Customizer recipes:

| Target | Recipe | Output |
|--------|--------|--------|
| `acl` | `acl.yaml` | QCOW2 |
| `acl-update` | `acl.yaml` | COSI |
| `acl-mshv` | `acl-mshv.yaml` | QCOW2 |
| `acl-mshv-update` | `acl-mshv.yaml` | COSI |

ACL-T comes from the latest completed, successful **scheduled** run of
`aclmain` (ACL pipeline **5304**) whose `acl-pipelines` checkout is
`refs/heads/main` and resolved `azure_container_linux` resource is
`refs/heads/aclmain`. Manual runs and other branches are excluded. The artifact is
`drop_build_acl_template_amd64_customize_and_sign`, file
`acl_template_image.vhdx`. These are native
ACL Btrfs/verity/OverlayFS images, not ordinary AZL images with altered identities.
All seven original partition GUIDs and sizes are retained. Image Customizer
regenerates the `/usr` UUID, verity, UKI and native addons on every build.

## Toolchain and RPMs

Use Linux with Docker, Go 1.26+, Python 3 with PyYAML and cryptography, and
authenticated `az devops` access to `https://dev.azure.com/mariner-org`.
The pinned ACL-enabled Image Customizer revision below is required; stock IC
does not implement its `acl.*` options. From the Trident repository root:

```bash
IC_SOURCE=/path/to/azure-linux-image-tools
ACL_CUSTOMIZER_PIN=$(python3 -c '
import yaml
with open(".pipelines/trident-ci.yml") as config:
    parameters = yaml.safe_load(config)["parameters"]
print(next(p["default"] for p in parameters
           if p["name"] == "ACL_CUSTOMIZER_PIN"))
')
test "$(git -C "$IC_SOURCE" rev-parse HEAD)" = "$ACL_CUSTOMIZER_PIN"
mkdir -p artifacts/acl-ic
(cd "$IC_SOURCE" && CGO_ENABLED=0 go build -mod=readonly -tags prod \
  -o "$OLDPWD/artifacts/acl-ic/imagecustomizer" ./toolkit/tools/imagecustomizer)
docker build -f tests/images/acl/Dockerfile -t acl-ic:local artifacts/acl-ic
```

Stage matching **AZL3** candidate RPMs in `bin/RPMS`: `trident`, `trident-acl`,
`trident-selinux` and `trident-static-pcrlock-files`. Both recipes remove and
reinstall them, so an installed newer version does not make replacement a
no-op. All four targets use the existing `requires_trident` setting to supply
`bin/RPMS` to Image Customizer. The base image's repositories remain enabled
for dependencies; no additional repository configuration is needed.
Keep a single matching candidate set, newer than publicly available Trident
packages, and do not mix AZL4 RPMs into this directory.

For MSHV, also stage the efitreeloader RPM from consumer **1261060**'s
`drop_build_efitreeloader_rpm` artifact in `artifacts/rpm-overrides`.
It contains loader revision `bf02d4e3f4`. The remaining MSHV packages, including
their licenses, are installed as RPMs from Microsoft's public repositories.
The recipe replaces `kernel` with `kernel-mshv`, pins compatible shim 15.8,
and assembles the ESP loader bundle in an inline customization hook.

## Build

```bash
python3 tests/images/testimages.py download-image acl-t
python3 tests/images/testimages.py build acl --container acl-ic:local --output-dir artifacts/acl
python3 tests/images/testimages.py build acl-mshv --container acl-ic:local --output-dir artifacts/acl-mshv
```

`download-image acl-t` refreshes the cached image. A missing image is downloaded
automatically; subsequent builds reuse it. Refresh once before building all
base/update variants, not between them.
The ACL COSI targets are excluded from `make all-cosi` because they require
the dedicated ACL toolchain and inputs.
The images enable `testuser` public-key SSH with passwordless sudo and DHCP,
select OEM `metal`, and disable the ACL agent. ACL's root-login denial remains.
Generated private keys are `key_acl_private.pem` and `key_acl-mshv_private.pem`.

For matching base/update keys, place the same public key at
`tests/images/acl/files/id_rsa.pub` before **each** build. The shared builder
consumes and removes that staging file. See [ACL Tests](../../../docs/Development/Testing/ACL-Tests.md)
for building and running a servicing pair.

## COSI publication

IC directly creates each COSI, including COSI 1.3 metadata with bootloader type
`efitreeloader-mshv-systemd-boot` for MSHV. The pinned revision includes native
MSHV boot-chain detection; no metadata rewriting or archive repacking is needed.
Publication copies the QCOW2, COSI and SSH key to `base.qcow2`, `update.cosi`
and `id_rsa` in the artifact directory. The private key is restricted to mode 0600.

Both variants built QCOW2/COSI pairs and passed content audits.
The initial standard native update, rollback, update, update, explicit commits
and a persistence reboot
passed in QEMU with verified Btrfs `/usr`. Validation used existing Trident
artifact 1256724 (`3fc019c`), not a fresh source-head RPM.
An earlier local MSHV boot reset before Linux after `second.efi`.
[Pipeline 1262480](https://dev.azure.com/mariner-org/ECF/_build/results?buildId=1262480&view=logs)
at source `a869fe163` subsequently passed the full MSHV servicing cycle with
four explicit commits on B/A/B/A and a persistence boot on A. This validates
host-cold-cycle servicing, not working warm reset, production signing or
cross-version hypervisor/loader servicing. See
[ACL Tests](../../../docs/Development/Testing/ACL-Tests.md) for transition details.
