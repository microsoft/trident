# Synthetic ACL images

These are **test-only Azure Linux 3.0 images**, not ACL releases. Both variants
exercise Trident's native ACL adoption and `/usr` dm-verity path. `standard`
uses the regular AZL kernel and systemd-boot. `mshv` uses `kernel-mshv` and
the EFI Tree Loader/HvLoader chain. Both variants use IPv4 DHCP so their
networking does not depend on a particular VM controller or static subnet.

## Image Customizer recipes

`standard-base.yaml`, `standard-update.yaml`, `mshv-base.yaml` and
`mshv-update.yaml` are complete, static Image Customizer configurations.
Layout and OS settings are duplicated deliberately; no inheritance, YAML
generation or generation-marker input is needed.

`prepare-synthetic-acl.sh` copies the recipes unchanged and supplies the
SSH key, exact pipeline Trident NEVRA and installed-package removal list.
Bash, guestfish, binutils, jq and GNU tar handle the ACL partition identities,
slot-specific UKI addons, COSI metadata and provenance. Payload downloads
also use Bash. `testimages.py` registers `synthetic-acl-standard` and
`synthetic-acl-mshv` as bundle targets and only dispatches to this Bash helper;
image configuration and post-processing remain outside Python. The CLI
requires Python 3, PyYAML and cryptography, like the existing VM image targets.

Run on a Linux image-build worker with sudo, Docker, libguestfs (`guestfish`),
RPM, OpenSSH, jq, GNU tar and binutils (`objcopy`, `objdump`) installed. The pinned
ImageCustomizer image must provide `/usr/bin/ukify`; the helper checks it
before building. Supply a **plain AZL 3.0 x86-64** base image and
the pipeline's RPM directory. The local ImageCustomizer Docker image must
support UKI creation, `/usr` verity and ACL `convert` (including `/boot` ESP).
The helper queries the base image's own RPM database and removes only installed
conflicting boot packages; tdnf rejects removal lists containing absent names.
Azure Linux's `dracut` package provides its systemd dracut modules: there is no
separate `dracut-systemd` RPM. The command fails rather than downloading a
different customizer version.

The test SSH override is included before Azure Linux's `PermitRootLogin no`.
Image construction checks host keys and effective root public-key-only login
with `sshd -t` and `sshd -T`; it does not enable root password login.

```bash
sudo tests/images/testimages.py build synthetic-acl-standard \
  --output-dir /outputs --no-download \
  --build-script-args \
  --base-image /inputs/azurelinux.qcow2 \
  --rpm-dir /inputs/trident-binaries \
  --ssh-public-key /inputs/ssh-keys/id_rsa.pub \
  --ssh-private-key /inputs/ssh-keys/id_rsa
```

Bundle outputs go under `/outputs/synthetic-acl-standard/` or
`/outputs/synthetic-acl-mshv/`. `list --filter-type bundle`, `dependencies`,
`show-image`, `list-files` and `build --dry-run` recognize both targets.
`--container` overrides the pinned IC 1.6 default. Put common CLI options
before `--build-script-args`; everything after it is passed unchanged to Bash.
Without overrides, the base and RPM paths remain `artifacts/baremetal.vhdx`
and `bin/RPMS`, as for other testimages targets. Supply the SSH key pair and,
for MSHV, the loader inputs described below. Cloning and force rebuilds are
not supported for bundles; use a fresh output directory.

The bundle directory must be empty. It contains `base.qcow2`, `update.cosi`,
the **exact** `metadata.json` stored in that COSI, `id_rsa` (0600), and
`manifest.json`. `.work/` retains build diagnostics/intermediates, not public
artifacts. Publish only the named artifacts, with private-key access restricted.
`metadataSha384` hashes metadata bytes, **not** the complete COSI archive.
`expectedTridentSha256` hashes the actual baseline `/usr/bin/trident`.
Both images pin the same pipeline RPM NEVRA; generation markers distinguish
the base and update's `/usr` contents. No ISO is produced.

For `mshv`, select `build synthetic-acl-mshv` and additionally supply
`--efitreeloader /inputs/efitreeloader.efi`,
`--mshv-payload /inputs/hypervisor`, and `--loader-provenance 'source SHA; profile SHA256'`.
The payload directory requires regular `HvLoader.efi`, `lxhvloader.dll` and
a nonempty `Windows/` component tree, copied without following links.
Obtain these components through an authorized HvLoader/MSHV build or artifact
source. This command does not invent a public payload URL or extract a nightly
ACL image. Exact source paths, hashes and caller provenance enter the manifest.

Build EFI Tree Loader from `https://github.com/microsoft/efitreeloader`
at `bf02d4e3f4b9a4b61e838fd4d3705a50573d1ce9` (merged PR 12), or a reviewed
descendant supporting `${{EFIDIR}}` expansion. In an independent checkout,
copy this directory's `load_entries.json` to `efitreeloader/load_entries.json`,
then run `cargo build --locked --target x86_64-unknown-uefi --release`.
Use `target/x86_64-unknown-uefi/release/efitreeloader.efi`. Older builds with
fixed EFI paths cannot service both slots and must not be used. The supplied
binary's build/profile identity is a caller trust boundary, not inferred from
its filename. Secure Boot must be disabled for these unsigned test artifacts.

## Layout and boot contract

Partitions: 1 EFI-SYSTEM, 2 USR-A, 3 HASH-A, 4 USR-B, 5 HASH-B, 6 OEM,
7 ROOT. ROOT is shared; only USR/HASH are A/B. The optional newer ACL hash
signature partitions are intentionally absent, which native adoption supports.
USR PARTUUIDs/types and HASH PARTUUIDs match `crates/sysdefs/src/acl.rs` and
`tools/cmd/mkcosi/generator/acl.go`. The ESP filesystem label is EFI-SYSTEM
and mounts at `/boot`; `/usr` is `/dev/mapper/usr`, ext4 and read-only.
Unlike production ACL, this synthetic test fixture does not include Ignition,
ACL's /etc overlay, BTRFS, release signing or production first-boot services.

ImageCustomizer generates the systemd initramfs and UKI. The helper moves its
fixed-slot verity arguments out of the common command-line section into real
`slot-a.addon.efi` and `slot-b.addon.efi` templates under `acl/uki-addons/` on
the ESP, matching `_uki_build_verity_addons` in ACL's
`build_library/rpm/uki_install.sh`. Each template contains this image's root
hash, its slot's fixed USR/HASH PARTUUIDs, panic-on-corruption and `acl.slot`.
Only slot A is initially active beside the UKI in its `.extra.d/` directory.
Trident replaces that addon with the target slot's template during staging;
it does not rewrite fixed PARTLABEL selectors. The common command line keeps
`mount.usr=/dev/mapper/usr` and uses `root=LABEL=ROOT` for the shared ROOT.
Both base and update images carry templates built from their own `/usr` hash.

The boot chain is shim -> grubx64.efi (systemd-boot), or for MSHV shim ->
grubx64.efi (ETL) -> HvLoader -> second.efi (systemd-boot). The latter's COSI
metadata is upgraded to 1.3 and `efitreeloader-mshv-systemd-boot` while retaining
the converter's UKI metadata and all original payload bytes/hashes. No Trident
parameters bypass native MSHV detection. Automatic boot-time commit is masked;
the test controller must explicitly commit the staged update.

## Manual QEMU servicing test

See [Synthetic ACL Tests](../../../docs/Development/Testing/Synthetic-ACL-Tests.md)
for local setup, fixture preparation, controller builds and diagnostics.

Both variants use the same storm scenario on a Linux KVM/libvirt worker:

```bash
sudo ./bin/storm-trident run synthetic-acl -a -- \
  --variant mshv --artifacts-dir /outputs/mshv --output-path /results/mshv
```

Use `--variant standard` for the regular kernel. MSHV requires exposed
VMX/SVM and enabled nested KVM; the scenario requests host CPU passthrough.
The guest must expose a character device at `/dev/mshv`, not merely boot a
`kernel-mshv` kernel. Both tests run A/B update, A/B rollback, A/B update,
then another A/B update, explicitly committing after each operation.
After each commit, `trident get status` must report `provisioned` and active
volumes B, A, B and A, respectively, using storm's shared status validator.
The initial boot checks slot A and the base image without querying Trident
status; the first update adopts the native ACL installation.
Each update separately stages and finalizes. Slot identity, image generation,
verity and retained boot-tree hashes are checked throughout, including one
further reboot after the last commit.
The scenario and assertions are Go code under `tools/storm/acl`. VM creation,
graceful shutdown/start, SSH/SFTP, Trident invocation and Host Status parsing
reuse storm's shared helpers. Both update and rollback separate staging from
finalization; staged checks require the running slot and boot ID to remain
unchanged. Go hashes slot boot files through privileged SFTP rather than
running guest shell assertions. Checkpoint measurements and complete command
output are saved on the controller before reboot and archived as
`guest-evidence.tar.gz`, so a failed boot cannot discard earlier evidence.
Updates use `internalParams.noTransition: true`; their complete command output
is captured in the controller log before an explicit reboot. Native rollback
restores the derived baseline configuration, which has no `noTransition`
override, so rollback retains its automatic reboot.
Failed runs retain the working disk and diagnostic output but remove the
owned VM. These prerequisites do not by themselves establish that a given
worker can boot the complete MSHV loader chain.

## Reproducible pipeline payload preparation

The helper downloads the three official Azure Linux nested-virtualization
packages using the exact URLs/SHA-256 in `mshv-packages.json` and preserves
their license/EULA files. No authentication or nightly image is required:

```bash
tests/images/synthetic-acl/fetch-mshv-payload.sh --output-dir /inputs/mshv-payload
```

Payload helper dependencies: Bash, curl, jq, `sha256sum`, `rpm2cpio`, `cpio`, HTTPS access to
packages.microsoft.com. Package bytes were downloaded and SHA-256 verified;
RPM payload paths were inspected. The main command's `--mshv-payload` points
at this directory; its manifest records every package, license and binary hash.
MSHV image preparation removes inherited shim before installing
`shim-15.8-6.azl3.x86_64`, matching Azure Linux's documented compatibility
constraint (shim 16.1 conflicts with mshv-bootloader-lx).

**EFI Tree Loader is currently a private Microsoft GitHub repository.** Add
an authorized pipeline repository resource/checkout, pinned to the revision
above; do not assume anonymous GitHub access. Copy that checkout to a build
directory, copy this profile over `efitreeloader/load_entries.json`, then build
inside `rust:1.98` with
`cargo build --locked --target x86_64-unknown-uefi --release` (the repository's
rust-toolchain.toml requests 1.98 and both UEFI targets). Supply the resulting
EFI binary to `--efitreeloader`; include source SHA and profile SHA-256 in
`--loader-provenance`. Access to that repository is the one authenticated
source prerequisite; official MSHV payload packages do not need credentials.

Validated ImageCustomizer 1.6 local digest:
`mcr.microsoft.com/azurelinux/imagecustomizer@sha256:db7bcb75be27cbab4face6dbaedd3b6e6c63c65c369f5763d81499ed843ccd44`.
