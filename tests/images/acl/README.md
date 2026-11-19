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
The producer's ACL-enabled Image Customizer revision is required; stock IC
does not implement its `acl.*` options. From the Trident repository root:

```bash
IC_SOURCE=/path/to/azure-linux-image-tools
test "$(git -C "$IC_SOURCE" rev-parse HEAD)" = c8dc566c0c04d33e2deaa6a484113e0545997f92
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
The images enable `testuser` public-key SSH with passwordless sudo and DHCP,
select OEM `metal`, and disable the ACL agent. ACL's root-login denial remains.
Generated private keys are `key_acl_private.pem` and `key_acl-mshv_private.pem`.

For matching base/update keys, place the same public key at
`tests/images/acl/files/id_rsa.pub` before **each** build. The shared builder
consumes and removes that staging file. See [ACL Tests](../../../docs/Development/Testing/ACL-Tests.md)
for building and running a servicing pair.

## COSI publication

IC directly creates each COSI. `publish-servicing-artifacts.sh` packages the
already-built QCOW2/COSI and SSH key for Storm; it does not customize images.
It verifies native ACL identity, distinct `/usr` UUIDs, identical candidate
Trident binaries and the compressed `/usr` hash, and records RPM/input hashes.
For MSHV it corrects IC's hardcoded `systemd-boot` metadata tag to COSI 1.3
`efitreeloader-mshv-systemd-boot`, preserving compressed partition bytes.
This workaround is restricted to these unsigned test artifacts.

Both variants built QCOW2/COSI pairs and passed content audits. Standard native
update, rollback, update, update, explicit commits and a persistence reboot
passed in QEMU with verified Btrfs `/usr`. Validation used existing Trident
artifact 1256724 (`3fc019c`), not a fresh source-head RPM.
Local MSHV still resets before Linux after `second.efi`;
neither `/dev/mshv` nor MSHV servicing is a demonstrated pass.
