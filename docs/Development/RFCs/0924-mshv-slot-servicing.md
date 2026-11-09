# 0924 Slot-Owned MSHV Boot Payloads

- Date: 2026-11-05
- RFC PR: [microsoft/trident#924](https://github.com/microsoft/trident/pull/924)
- Issue: Not created.
- Status: Draft proposal; not implemented or boot-validated.

## Summary

Enable Trident to install, update and roll back MSHV-enabled Azure Container
Linux (ACL) images by keeping a complete hypervisor boot payload beside each
slot's EFI chain. Preserve existing firmware and UKI selection. Extend the
existing Rust slot validation for applicable MSHV boots so a mixed
hypervisor/OS boot cannot be committed.

Update efitreeloader to enable slot-owned MSHV boot payloads.

## Motivation and Goals

### Current boot chain

The inspected MSHV image uses the following chain. Paths are relative to the
EFI System Partition (ESP), regardless of its Linux mount point.

```text
UEFI -> \efi\BOOT\bootx64.efi       shim
     -> \efi\BOOT\grubx64.efi      efitreeloader, not GRUB
        -> \efi\HvLoader.efi
           -> \efi\lxhvloader.dll
              -> \efi\Windows\System32\... hypervisor payload
        -> \efi\BOOT\second.efi   systemd-boot
           -> selected UKI -> Linux
```

Efitreeloader currently has a compiled-in set of absolute paths that are
used at runtime to enable MSHV boot. These paths are configurable at build
time via efitreeloader's JSON configuration.

The loader already resolves relative child executable paths against its
executing directory using `LoadedImageDevicePath`. However, the current
HvLoader/DLL arguments name fixed absolute DLL and hypervisor-root paths.
Moving only the EFI chain therefore does not make the entire payload
slot-owned.

HvLoader verifies and loads the external DLL. Its public source describes
preparing the hypervisor without running it and registering a kernel-facing
protocol.

[microsoft/trident#915](https://github.com/microsoft/trident/pull/915)
proposes preserving the existing shared-path payload during clean install.
That is useful initial enablement, but because it overwrites the "shared"
MSHV payload, it does not provide a dependable rollback.

### Problem

An A/B update must not overwrite the active slot's hypervisor files.
Otherwise, reverting the OS can leave it using the updated release's
hypervisor. The ESP is shared storage, so this requires explicit ownership
and copying.

There are also two independent boot selectors:

| Layer | Selection |
| --- | --- |
| Firmware | `BootNext`/`BootOrder` select a `Boot####` entry containing an ESP device path and a path such as `\EFI\AZLB\bootx64.efi`. |
| Systemd-boot | `LoaderEntryOneShot`/`LoaderEntryDefault` select a UKI in shared `EFI/Linux`. `LoaderEntrySelected` reports the selected entry. |

These selectors can diverge. For example, an A-to-B trial selects B's EFI
chain and B's UKI. If B fails **before** systemd-boot consumes the UKI oneshot,
firmware may try A's chain, prepare A's hypervisor, then start B's UKI.
Slot-owned payloads introduce a compatibility consequence to this existing
selector divergence.

The proposal preserves those selectors. Its guarantee is **no successful
commit of an observed mixed boot**, not prevention of every transient
mixed launch.

### Goals

- Service the hypervisor payload with the OS using the existing image and
  Trident lifecycle.
- Retain the committed payload unchanged while staging an update, and reuse
  it for automatic and manual rollback.
- Keep efitreeloader out of UKI selection.
- Detect inconsistent hypervisor/UKI/root identity before default or
  fallback promotion.
- Preserve existing behavior for non-MSHV and legacy images.

## Scope

### Requirements

- Build 1ES COSI files with the required MSHV content and an A/B-update-compatible
  structure.
- Support stage, finalize and commit for installation and update of 1ES MSHV
  image COSI files.
- Support automatic rollback for failed updates.

### Out of Scope

- General MSHV support; this feature is gated by the internal parameter
  `mshvSlotEspPayload`, and only tested for specific 1es images.

### Unsure

- Initial testing uncovered that 2 1es images built from the same ACL-T base
  image contain /usr partitions that share the same btrfs fs uuid, but differing
  verity hashes (reflecting different content). Trident cannot handle this in
  v0.30.0. We need to either 1) update the 1es builds (and/or ImageCustomizer)
  to ensure that the btrfs fs uuid is unique for each /usr partition, or 2) explicitly
  ensure that we do not publish this conflicting combination of /usr partitions. I
  think we want `#1` and this should be in-scope.

### Exit Criteria

Clean install, A/B update, commit, failed-trial recovery and manual rollback
are demonstrated with distinguishable A/B payloads and matching UKIs/root
devices. A mixed boot reaching trident commit fails. Unsupported
transitions, invalid evidence and insufficient space fail explicitly.
Secure Boot, actual DLL relocation, configured fallback and supported
physical-firmware behavior are validated. TPM/PCR-bound encryption remains
rejected until its additional measurement and unlock coverage passes.

## Dependencies

- **Efitreeloader:** implement one resolver alternative below.
- **HvLoader and external loader DLL:** demonstrate that ESP-absolute DLL
  and root paths work in each slot and fallback directory, without silently
  substituting fixed shared payloads.
- **Image producer:** build and sign the loader, assemble the complete
  relocatable ESP bundle, and deliver it through the image's existing ESP
  content. No COSI schema revision is proposed.
- **Trident:** extend ESP copying and the existing Rust boot-slot validation
  for applicable MSHV boots.
  Coordinate with microsoft/trident#915's pending legacy clean-install
  behavior.

## Implementation

### Slot-owned ESP layout

The source image supplies one canonical bundle under `EFI/BOOT`. Trident
copies it into the target slot directory.

See the [image-build changes](#a2-1es-image-build) for source bundle assembly.

```text
ESP/
  EFI/
    AZLA/
      bootx64.efi                 shim
      grubx64.efi                 efitreeloader
      second.efi                  systemd-boot
      HvLoader.efi
      lxhvloader.dll
      Windows/System32/
        hvix64.exe
        hvax64.exe
        kdstub.dll
    AZLB/
      ...independently versioned bundle...
    BOOT/
      ...configured fallback copy...
    Linux/
      vmlinuz-100-azla0.efi
      vmlinuz-100-azla0.efi.extra.d/
      vmlinuz-101-azlb0.efi
      vmlinuz-101-azlb0.efi.extra.d/
```

Validate the complete allowlisted tree as regular files/directories,
rejecting links, special files and unexpected content. The only payload
directory tree is `Windows/System32`.

UKIs remain in shared `EFI/Linux`; their existing `.extra.d` staging and
ACL slot/firstboot policies remain unchanged. `AZL2B` is an additional
installation's EFI directory and corresponds to UKI suffix `azlb1`, not
`azlb2`. Reuse Trident's naming helpers.

### Efitreeloader MSHV payload resolution

Updating efitreeloader to enable slot-owned MSHV payloads could be
achieved through several approaches. Three approaches are outlined
[below](#efitreeloader-resolver-alternatives).

I prefer [**Alternative A**](#alternative-a-executing-loader-directory-prefix),
which uses the executing efitreeloader binary's directory to
resolve the MSHV payload.

### Extend existing slot validation for MSHV

Extend the existing slot check in
`crates/trident/src/engine/rollback.rs::validate_boot`, which currently
compares the booted root device with the expected target. When the
slot-owned MSHV feature applies, also compare the running hypervisor's
slot with the booted OS slot and expected servicing target.

Keep existing slot-check behavior unchanged for other images.

This is an expansion of boot-slot validation, but implementation is
TBD. The [implementation boundaries](#a4-remaining-integration-work) identify
the commit and rollback paths that need this expanded slot check.

Existing `LoaderEntrySelected`, root/verity validation and the
expected servicing target identify the booted OS. Use `BootCurrent` as
corroboration, not an arbitrary label as proof of the executed hypervisor
chain.

Preparing A's hypervisor with a B UKI/root is a mismatch, but the existing
OS/firmware observations alone do not prove which hypervisor is running.
The mechanism for identifying the running hypervisor's slot remains TBD;
the location of the comparison in existing slot validation is not TBD.
This RFC does not specify a replacement reporting protocol.
The executing directory `EFI/BOOT` alone also does not establish which
slot's payload was copied there. Until a metadata-free attribution
mechanism is defined and tested, reject ambiguous BOOT promotion.

Perform the expanded slot validation before user health hooks and before
`BootOrder`, `LoaderEntryDefault` or fallback promotion.

Propose `ServicingError::ValidateAbActiveHypervisor`, following the existing
`ServicingError::ValidateAbActiveVolume` pattern. Return it from the expanded
slot validation when the active hypervisor does not match the expected
slot or required hypervisor identity cannot be established. Include
expected and observed identity context where available; exact fields and
the identification mechanism remain TBD.

The validation error rejects a mixed boot before commit. The
[auto-rollback scaffolding](#scaffolding-for-invalid-hypervisor-auto-rollback)
below describes the additional state and dispatch needed to initiate and
track automatic recovery; returning the validation error alone does not
provide that behavior.

Clean install reports provisioning failure without declaring success.
Apply the expanded slot check to manual rollback and automatic
rollback-recovery success. Do not declare the machine provisioned when MSHV
identity evidence is missing or contradictory. Hypervisor identification
and error fields remain implementation review items.

Existing firmware promotion uses the label of `BootCurrent`. An explicit
BOOT entry therefore needs deliberate mapping to the validated managed
entry. Until that is defined and tested, reject ambiguous promotion rather
than accidentally promoting BOOT.

### Scaffolding for invalid hypervisor auto-rollback

The following is proposed scaffolding for automatically rolling back an
update with an invalid hypervisor, not existing active-volume mismatch
handling.

For an A/B update with an unexpected hypervisor or missing/contradictory
required MSHV identity evidence, persist
`ServicingError::ValidateAbActiveHypervisor` and a new failure state,
provisionally `AbUpdateHypervisorCheckFailed`, before initiating rollback.
Reuse rollback actions where appropriate, but explicitly extend
commit, reboot and recovery dispatch to recognize the new state.

Recovery from the new state expects the retained pre-update OS slot and
its matching hypervisor. Return to `Provisioned` only after the expanded
slot check validates both, preserving/reporting the original
unexpected-hypervisor error. If recovery is still mixed or cannot establish
the required identity, retain the failure state and report recovery failure
rather than promoting either payload or repeatedly oscillating slots.

### Lifecycle

| Phase | Behavior |
| --- | --- |
| Clean install | Stage A's complete bundle and UKI with existing ACL verity/firstboot handling. Register/finalize using existing selectors; do not invent a usable B chain. Validate identity before success. |
| A-to-B update stage | Stage only B's OS, UKI/addons and complete payload. Preserve A and strip the image firstboot addon as today. |
| Update finalize | Complete payload publication before setting existing B firmware/UKI trial selectors. Keep committed A defaults and existing fallback-mode decisions. |
| Trial and commit | Extend existing slot validation with the applicable MSHV comparison; require matching hypervisor/UKI/root identity and existing health checks before promoting firmware default, UKI default or fallback. |
| Unexpected hypervisor during trial | Persist `ServicingError::ValidateAbActiveHypervisor` and the proposed `AbUpdateHypervisorCheckFailed` state, then initiate rollback without promoting B. With B's oneshot consumed and committed A defaults intact, the next reset should restore A+A; verify those conditions. |
| Health-check failure during trial | Retain existing `AbUpdateHealthCheckFailed` handling; do not conflate it with MSHV slot-validation failure. |
| Recovery after hypervisor-validation failure | Recognize the new state, validate retained A's OS and hypervisor before returning to `Provisioned`, and preserve/report the original error. A failed recovery retains the failure state. |
| Manual rollback | Select retained A using existing firmware/UKI logic. Validate A independently of whether a new UKI is staged, and require matching identity before success. |
| Later slot reuse | Replace the old inactive bundle only after the other slot is committed. Never refresh the rollback payload from the newer image. |

A boot that hangs before Linux cannot be observed by commit. Reuse existing
firmware fallback and platform watchdog/external reset behavior. Reset does
not itself clear an unconsumed `LoaderEntryOneShot`; do not promise A+A
recovery after every pre-systemd failure. Repeated mismatch must report
failure rather than oscillate slots. Add a narrow recovery change only if
testing demonstrates a new MSHV-specific failure.

### UEFI fallback

The [Trident diff](#a3-trident-payload-deployment-and-fallback) adds a gated
nested-file replacement path without changing legacy `replace_boot_files`.

Copy the chosen slot's complete bundle, including `Windows/System32`,
to `EFI/BOOT`. Resolve runtime paths to BOOT's own files.
This is a payload snapshot, **not** a self-contained fallback OS: UKIs stay
shared, and no copied exact-UKI selector is added.

Preserve existing fallback modes:

| Mode | During B trial | After successful B commit |
| --- | --- | --- |
| Conservative | Retain A payload in BOOT. | Copy B payload. |
| Optimistic | Copy B payload to BOOT. | Retain B payload. |
| Disabled | Do not create/update fallback. | Unchanged. |

An MSHV fallback-copy failure has the same state/error behavior as ordinary
fallback-copy failure.

Exercise optimistic B fallback surviving a failed trial and later B restage.
A mixed fallback boot reaching Linux must fail validation; an ambiguous
BOOT-to-managed promotion remains unsupported.

## Public API Design

No new public Host Configuration API or COSI metadata revision is proposed.
The provisional internal flag is off by default and requires validation of
the payload files.

Efitreeloader build-time configuration changes are described above.

The [auto-rollback scaffolding](#scaffolding-for-invalid-hypervisor-auto-rollback)
proposes `AbUpdateHypervisorCheckFailed` as a new persisted Host Status
value, distinct from `AbUpdateHealthCheckFailed`.
> Note: it should be verified that this change will not cause some trident conflicts
> when trident reads a datastore containing this new HostStatus state.

## Testing and Metrics

### Focused tests

| Area | Coverage |
| --- | --- |
| Executing-directory prefix (A, preferred) | Own image handle/device path, parent-directory normalization, exact substitution, argument lifetime/encoding, malformed/missing paths, multiple ESPs, renamed binaries, fallback and chainloading without depending on `BootCurrent`. |
| Literal compatibility (A/B) | Build-time detection of the exact token; absolute, relative, empty and nonmatching-token arguments remain unchanged. Literal-only configurations do not trigger prefix resolution or its additional validation. |
| Boot-entry prefix (B) | Binary load-option/device-path parsing, UTF-16 paths, device identity, malformed/missing variables and executing-path mismatch; preserve shared template guarantees. |
| Profiles (C) | Legacy compatibility, strict schema, duplicate/malformed roots, zero/multiple matches, case handling, ordering and unchanged argument bytes; unlisted roots fail before child execution. |
| Payload deployment | Complete allowlist, links/special-file rejection, inactive-only writes, slot reuse, firstboot/addon preservation, insufficient space, failed-copy retries and non-MSHV compatibility. |
| Expanded slot check | MSHV-only A+B and B+A mismatch, higher install-index naming, missing or contradictory identity evidence and rejection of ambiguous BOOT promotion; preserve existing root-slot mismatch behavior and verify MSHV failures enter the new hypervisor-failure state without invoking user health hooks. Hypervisor identification remains TBD. |
| Failure-state integration | Verify `ServicingError::ValidateAbActiveHypervisor` propagates like `ValidateAbActiveVolume` while update handling explicitly persists the distinct hypervisor-failure state; distinguish it from `AbUpdateHealthCheckFailed`, exercise command/daemon commit and reboot dispatch, validate recovery against the retained slot and preserve the original error. Failed recovery must not mark the host provisioned. |
| Fallback | Nested files; failures during copy/rename/cleanup; stale exact `.new`/`.old` entries; same servicing errors as ordinary fallback failures and no active-slot mutation. |

### VM and firmware acceptance

| Scenario | Required evidence |
| --- | --- |
| Install A, update B, commit B | Distinct A/B payloads, matching hypervisor/UKI/root identity; A unchanged during B trial. |
| B EFI failure before oneshot consumption | Inject A+B launch; if Linux starts, fail before promotion and verify A+A after consumed-oneshot recovery with intact defaults. |
| Failure after oneshot consumption; health failure | Verify retained payload, existing selector recovery and no promotion after failed health validation. |
| Hypervisor mismatch at commit, then recovery | Persist the new hypervisor-failure state before rollback; after restart recognize that state and validate the retained OS/hypervisor pair. Preserve the original error and reject mixed or unidentifiable recovery. |
| Pre-Linux hang/failure | Exercise firmware/watchdog/external reset; document unconsumed-oneshot behavior without claiming OS health checks ran. |
| B commit followed by manual rollback A | Retained A files and validated A hypervisor/UKI/root identity before success. |
| Conflicting defaults, menu selection, chainloading | Reject mixed identity or unsupported resolver attribution. |
| Entry loss or explicit BOOT entry | Reject promotion when fallback payload identity cannot be established. |
| Fallback modes and repeated slot reuse | Conservative/optimistic/disabled behavior; failed-B fallback followed by restaging does not bypass validation. |
| Copy/publication/finalization interruption | Compare existing failure/retry behavior, preserve committed payload and do not advance servicing state incorrectly. |
| Secure Boot | Approved signed loader, signer/SBAT policy and retained downstream verification; exact packaged DLL loads the intended Intel/AMD payload. |
| Measurement and encryption | Capture actual event log and validate prediction/unlock before allowing TPM/PCR-bound encryption. |
| Legacy or unsupported images | Preserve existing paths, or reject unsupported conversion before modifying the target. |

Run Trident `functional-test` cases only inside VMs, never on a bare build
host. OVMF results alone do not establish supported physical-firmware
behavior. Inspect the built image/COSI and run the image workspace's
existing validation before boot testing.

Use existing structured servicing errors and rollback handling to report
preparation failure, boot-slot identity mismatch, unsupported layout/resolver
and capacity failure, with `ServicingError::ValidateAbActiveHypervisor` and a
failure state for applicable A/B updates. Capture selected UKI, executing
image directory and
firmware entry in diagnostic logs. No new telemetry service or attestation metric is
proposed.

### Measured boot and ESP capacity

Current PCR prediction recognizes primary `bootx64.efi` and secondary
`grubx64.efi` bootloader names. This chain adds stages after efitreeloader.
Capture actual TCG events and update prediction/callers for the observed
ordering and measurements; do not assume every DLL/EXE is measured into
PCR 4. Reject TPM/PCR-bound encryption until encrypted VM tests pass.

A November 5 package-manifest measurement gives representative allocation
for the five hypervisor files, **not the exact target image**:

| Files | Allocation at 4 KiB FAT clusters |
| --- | ---: |
| HvLoader, loader DLL, Intel/AMD hypervisors and debug stub; one copy | 6.457 MiB |
| Three copies: A, B and BOOT | 19.371 MiB |
| Extra over one shared copy | 12.914 MiB |

Inputs were Azure Linux 3.0
`edk2-hvloader-20240524git3e722403cd16-21.azl3.x86_64`,
`mshv-26100.9444.2609032029.1-1.azl3.x86_64` and
`mshv-bootloader-lx-26100.9444.2609032029.1-1.azl3.x86_64`.
The figures exclude shim, efitreeloader, systemd-boot, UKIs/addons,
filesystem metadata and temporary replacement copies.

Measure actual old/new bundles and FAT allocation. Preflight retained and
simultaneous staged/previous bundles, retained/staged UKIs/addons,
unrelated contents, filesystem metadata and a safety reserve. Do not infer
that a 128 MiB ESP is sufficient or prescribe a replacement size from
these five-file figures alone. Increase configured ESP capacity when the
actual peak requires it; never reclaim the committed slot to fit a trial.

## Servicing

Enable the feature only for validated new-layout images and capable
Trident/loader versions. Do not silently convert existing shared-path MSHV
installations or mix a legacy A rollback baseline with slot-owned B.
Keep microsoft/trident#915's legacy preservation behavior separate.

Reject raw-image/disk-streaming transitions initially. Supporting them
later requires full-tree bootstrap, the expanded MSHV slot validation
and independently tested rollback/fallback behavior.

An image must not be offered for this layout to an older consumer that
does not understand its layout or MSHV slot-validation extension.
Establish minimum producer/loader/Trident versions in the release pipeline;
do not assume older binaries reject unsupported images automatically.
Removing the opt-in must not bypass validation of an already managed MSHV
boot; define persisted layout detection and reject unsafe disablement.

## Implementation Plan

1. Implement the chosen generic loader resolver and image bundle.
   Verify exact arguments and actual hypervisor preparation.
2. Define reliable hypervisor identity evidence without a new temporary
   UEFI variable, then extend the existing Rust slot check with the
   applicable MSHV comparison. Add the distinct hypervisor-failure servicing
   state/error and extend rollback/recovery dispatch, including mixed trial,
   manual rollback and recovery. Preserve existing root-slot and health-check
   failure behavior; do not add an implicit health check.
3. Extend inactive-slot and fallback copying with existing state/error
   semantics; test interruption, retry and slot reuse.
4. Complete Secure Boot, measurements, capacity and supported firmware
   validation, then enable the opt-in in production image pipelines.

## Efitreeloader resolver alternatives

My preference is **Alternative A** as it seems the most straightforward and simple
for resolving the template prefix.

* [Alternative A](#alternative-a-executing-loader-directory-prefix) uses a build-time template
  and runtime resolution using the executing binary's directory.
* [Alternative B](#alternative-b-firmware-boot-entry-prefix) uses a build-time template and
  runtime resolution based on the firmware path contained in the current boot entry.
* [Alternative C](#alternative-c-complete-signed-argument-strings) expands the existing build-time
  path specification json to include multiple absolute paths, chosen at runtime.

### Alternative A: executing-loader directory prefix

See the [proposed loader diff](#a1-efitreeloader-template-expansion).

Compile one argument template into the signed efitreeloader. Efitreeloader,
not HvLoader or the DLL, expands the designated `${{PREFIX}}` token before
invoking the child.

Use `LoadedImageDevicePath` on efitreeloader's own image handle, as the
loader already does for relative child executable paths. Set `${{PREFIX}}`
to the parent directory of the executing binary, not its filename, the
firmware boot entry's path or the Linux ESP mount point.

```json
{
  "load_entries": [
    {
      "path": "HvLoader.efi",
      "args": "${{PREFIX}}\\lxhvloader.dll MSHV_ROOT=\\${{PREFIX}}\\Windows MSHV_ENABLE=TRUE MSHV_SCHEDULER_TYPE=ROOT MSHV_X2APIC_POLICY=ENABLE MSHV_SEV_SNP=TRUE MSHV_LOAD_OPTION=INCLUDETRACEMETADATA=1"
    },
    { "path": "second.efi" }
  ]
}
```

```text
Executing efitreeloader = ESP device path + \EFI\AZLB\grubx64.efi
  -> ${{PREFIX}} = \EFI\AZLB
  -> DLL argument = \EFI\AZLB\lxhvloader.dll
     MSHV_ROOT=\\EFI\AZLB\Windows
```

Reuse the same loaded device/ESP and directory for child executable
resolution and argument expansion. Normalize the directory to one leading
backslash and no trailing backslash; validate the supported path before
starting any child. Missing, malformed or ambiguous loaded-image paths
must fail closed.

The exact `${{PREFIX}}` token in a child's compiled `args` is the opt-in;
there is no separate JSON flag. Resolve and validate the prefix only if at
least one child needs expansion.

Without that token, pass the compiled arguments unchanged, including full
absolute paths, relative arguments and empty strings. Do not apply new
prefix validation to literal-only configurations. Only the exact token is
reserved; other text remains literal.

### Alternative B: runtime boot-entry prefix

Use the same compiled argument template and token-based opt-in as Alternative
A, but derive `${{PREFIX}}` from the firmware-selected executable instead
of efitreeloader's own location:

```text
BootCurrent = 0008
  -> Boot0008 = ESP device path + \EFI\AZLB\bootx64.efi
  -> ${{PREFIX}} = \EFI\AZLB
```

Read `BootCurrent` as a binary 16-bit value and parse the corresponding
`Boot####` EFI load option. Both variables use global-variable GUID
`8be4df61-93ca-11d2-aa0d-00e098032b8c`. Extract the executable's parent
from its device path, not its description or `BootNext`/`BootOrder`.
The variable contains a concrete path, not the placeholder.

Preserve the device/ESP identity and corroborate it and the directory
against the executing chain's loaded-image context before loading children.
`BootCurrent` identifies the firmware option, not every later chainloaded
executable. Missing/malformed variables, unsupported paths or disagreement
must fail closed; test firmware fallback and chainloading explicitly.
This is exact token substitution, not shell expansion or runtime JSON input.

### Alternative C: signed directory profiles

Compile a generic list of `{root, load_entries}` profiles into the signed
loader. Each profile contains complete child argument strings.

The following illustrates B's profile only. A deployable image must also
cover A and configured BOOT fallback.

```json
{
  "profiles": [
    {
      "root": "\\EFI\\AZLB",
      "load_entries": [
        {
          "path": "HvLoader.efi",
          "args": "\\EFI\\AZLB\\lxhvloader.dll MSHV_ROOT=\\\\EFI\\AZLB\\Windows MSHV_ENABLE=TRUE MSHV_SCHEDULER_TYPE=ROOT MSHV_X2APIC_POLICY=ENABLE MSHV_SEV_SNP=TRUE MSHV_LOAD_OPTION=INCLUDETRACEMETADATA=1"
        },
        { "path": "second.efi" }
      ]
    }
  ]
}
```

Reject duplicate/malformed roots, empty profiles and documents mixing the
legacy and profile forms at build time. Reject zero or multiple runtime
matches before starting any child. Retain the existing top-level
`load_entries` schema as a separate legacy configuration.

This bounds runtime selection to complete signed strings. Additional roots
such as `AZL2A`/`AZL2B` require explicit profiles and release coverage.
Reject unsupported install indices before deployment; an unsigned manifest
must not expand the signed root list.

For all three alternatives, preserve the current extra-leading-backslash
`MSHV_ROOT` convention and validate it with the packaged DLL. Keep argument
storage alive through child execution and preserve its encoding.


## Open Questions

- Does HvLoader, or its external loader DLL, load anything beyond the inputs
  passed by efitreeloader as arguments? If so, are those paths relative or
  absolute, and does HvLoader's location affect how they are resolved?
- How can Trident establish the running hypervisor's slot identity?
- Is new HostStatus API needed?  What impact as far as trident backward and
  forward compatibility does that carry?
- What persisted layout/identity state prevents unsafe opt-out and supports
  expected-payload diagnostics through commit and rollback?  For example, if
  the active OS is the VHD-based initial OS, does it have anything in its
  HostStatus identifying it as mshv-enabled?  Do we want to validate during
  update that it has the requisite mshv payload in the appropriate location?
  How much error checking do we want to include?
- Which minimum component versions, install roots and firmware platforms
  are supported at launch? What measured peak ESP size do their images need?
- For encryption sealed with a TPM, how are the MSHV components measured
  and their expected PCR values predicted? What changes are needed in the
  existing measurement/prediction flow?

## References

Source observations are pinned baselines, not claims about deployment:

- [Trident boot entries at dc5a201b](https://github.com/microsoft/trident/blob/dc5a201b8abcdd849252fed0814596a75c8c2749/crates/trident/src/engine/bootentries.rs)
  and [EFI variable helpers](https://github.com/microsoft/trident/blob/dc5a201b8abcdd849252fed0814596a75c8c2749/crates/osutils/src/efivar.rs).
- [MSHV clean-install preservation, microsoft/trident#915](https://github.com/microsoft/trident/pull/915),
  inspected at `c01e4b1c7aa54c4514bd5074f6fd928811e71e00`.
- [Efitreeloader at 62859b9f](https://github.com/microsoft/efitreeloader/tree/62859b9f8b9fa7a5a37cc5f554756524b4d020a5),
  including `entrypoint.rs`, `devicepath.rs` and compiled configuration.
- [ACL MSHV image recipe](https://dev.azure.com/mariner-org/ACL/_git/acl-baremetal?path=/1es/acl/by-virt/mshv.yaml&version=GC6c67ec657279ff9c8e7513fc05c4e6be49d104af)
  and [loader build script](https://dev.azure.com/mariner-org/ACL/_git/acl-baremetal?path=/tools/build-efitreeloader.sh&version=GC6c67ec657279ff9c8e7513fc05c4e6be49d104af).
- [HvLoader source](https://github.com/microsoft/HvLoader/blob/main/HvLoader.c)
  and [external loader packaging](https://github.com/microsoft/openvmm-deps/blob/main/pkg/hvloader/README.md).

## Appendix A: Proposed code changes

These are concrete source-based diffs for the preferred template approach,
not changes to source files in this RFC PR. They cover argument resolution,
image assembly and gated payload copying. They are **not a complete
release implementation**: a reliable hypervisor identity mechanism,
expanded MSHV slot validation, capacity checks and release validation remain
prerequisites, listed in [A.4](#a4-remaining-integration-work). Applying these
diffs alone must not be treated as production enablement.

Baselines:

| Repository | Revision |
| --- | --- |
| `microsoft/efitreeloader` | `62859b9f8b9fa7a5a37cc5f554756524b4d020a5` |
| ADO `ACL/acl-baremetal` | `cd2dd9e46d3715671112e12a67aff907f68a0ca4` |
| `microsoft/trident` | `dc5a201b8abcdd849252fed0814596a75c8c2749` |

### A.1 Efitreeloader template expansion

Related design: [executing-loader directory prefix](#alternative-a-preferred-executing-loader-directory-prefix).
The build script derives an internal flag from the exact `${{PREFIX}}`
token in each child's arguments; JSON does not expose that flag. Resolve
the prefix from the existing loaded-image parent before starting any child
only when at least one entry needs expansion.
The initial implementation accepts ASCII directories under `EFI` and
rejects malformed or ambiguous paths. This diff implements Alternative A,
not the firmware-variable resolver in B.

```diff
diff --git a/efitreeloader-lib/src/devicepath.rs b/efitreeloader-lib/src/devicepath.rs
index 9d7745c..684c66b 100644
--- a/efitreeloader-lib/src/devicepath.rs
+++ b/efitreeloader-lib/src/devicepath.rs
@@ -6,6 +6,8 @@ mod tests;

 use alloc::borrow::ToOwned;
 use alloc::boxed::Box;
+use alloc::format;
+use alloc::string::ToString;
 use alloc::vec::Vec;
 use uefi::fs::PathBuf;
 use uefi::proto::device_path::build::{self, DevicePathBuilder};
@@ -19,6 +21,29 @@ pub(crate) struct DeviceFilePath {
     file_path: CString16,
 }

+pub(crate) fn executing_prefix(executing: &DeviceFilePath) -> uefi::Result<CString16> {
+    let invalid = || Error::new(Status::INVALID_PARAMETER, ());
+    let parent_path = executing.file_path.to_string();
+    let prefix = format!("\\{}", parent_path.trim_start_matches('\\'));
+    let segments: Vec<_> = prefix.split('\\').collect();
+    if segments.len() < 3
+        || !segments[0].is_empty()
+        || !segments[1].eq_ignore_ascii_case("EFI")
+        || segments[1..].iter().any(|segment| {
+            segment.is_empty()
+                || *segment == "."
+                || *segment == ".."
+                || !segment
+                    .bytes()
+                    .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
+        })
+        || executing.hardware_path.node_iter().next().is_none()
+    {
+        return Err(invalid());
+    }
+    CString16::try_from(prefix.as_str()).map_err(|_| invalid())
+}
+
 // Verifies a UEFI DevicePath matches the expected format (i.e. at least one media file path at the end), then gets the
 // parent directory.
 pub(crate) fn get_device_path_parent_dir(device_path: &DevicePath) -> uefi::Result<DeviceFilePath> {
diff --git a/efitreeloader-lib/src/entrypoint.rs b/efitreeloader-lib/src/entrypoint.rs
index a80a59e..eff31aa 100644
--- a/efitreeloader-lib/src/entrypoint.rs
+++ b/efitreeloader-lib/src/entrypoint.rs
@@ -1,6 +1,8 @@
 // Copyright (c) Microsoft Corporation.
 // Licensed under the MIT License.

+use alloc::string::ToString;
+use uefi::CString16;
 use uefi::boot::LoadImageSource;
 use uefi::proto::BootPolicy;
 use uefi::proto::device_path::{DevicePath, LoadedImageDevicePath};
@@ -10,7 +12,7 @@ use uefi::runtime::VariableAttributes;
 use uefi::{Error, Identify, Status, boot, cstr16, println, runtime};

 use crate::LoadEntry;
-use crate::devicepath::{get_device_path_parent_dir, join_device_file_path};
+use crate::devicepath::{executing_prefix, get_device_path_parent_dir, join_device_file_path};

 // The efitreeloader application logic.
 pub fn entrypoint(load_entries: &[LoadEntry]) -> uefi::Result {
@@ -27,6 +29,12 @@ pub fn entrypoint(load_entries: &[LoadEntry]) -> uefi::Result {
             .inspect_err(|_| println!("Error: Failed get parent directory of EFI file."))?;
     }

+    let prefix = if load_entries.iter().any(|entry| entry.expand_boot_prefix) {
+        Some(executing_prefix(&parent_path)?)
+    } else {
+        None
+    };
+
     if cfg!(debug_assertions) {
         println!("Entries:");
         for entry in load_entries {
@@ -57,9 +65,18 @@ pub fn entrypoint(load_entries: &[LoadEntry]) -> uefi::Result {
         )
         .inspect_err(|_| println!("Error: Failed to load image: {}", file_path))?;

-        if !target.args.is_empty() {
+        let expanded_args = if target.expand_boot_prefix {
+            let prefix = prefix.as_ref().ok_or(Error::new(Status::INVALID_PARAMETER, ()))?;
+            let expanded = target.args.to_string().replace("${{PREFIX}}", &prefix.to_string());
+            Some(CString16::try_from(expanded.as_str()).map_err(|_| Error::new(Status::INVALID_PARAMETER, ()))?)
+        } else {
+            None
+        };
+        let args = expanded_args.as_deref().unwrap_or(target.args);
+
+        if !args.is_empty() {
             let mut loaded_image = boot::open_protocol_exclusive::<LoadedImage>(image_handle)?;
-            let args_bytes = target.args.as_bytes();
+            let args_bytes = args.as_bytes();
             let args_bytes_len: u32 = args_bytes.len().try_into().map_err(|_| {
                 println!("Error: Args for {} are too long.", file_path);
                 Error::new(Status::INVALID_PARAMETER, ())
diff --git a/efitreeloader-lib/src/loadentries.rs b/efitreeloader-lib/src/loadentries.rs
index 25c1181..23f88d9 100644
--- a/efitreeloader-lib/src/loadentries.rs
+++ b/efitreeloader-lib/src/loadentries.rs
@@ -8,4 +8,5 @@ use uefi::CStr16;
 pub struct LoadEntry {
     pub path: &'static CStr16,
     pub args: &'static CStr16,
+    pub expand_boot_prefix: bool,
 }
diff --git a/efitreeloader/build.rs b/efitreeloader/build.rs
index 7f0bfa7..9dacad9 100644
--- a/efitreeloader/build.rs
+++ b/efitreeloader/build.rs
@@ -45,6 +45,7 @@ fn handle_load_entries() {
         write!(dest_file, "    ::efitreeloader_lib::LoadEntry{{\n").unwrap();
         write!(dest_file, "        path: ::uefi::cstr16!(\"{}\"),\n", entry.path.escape_debug()).unwrap();
         write!(dest_file, "        args: ::uefi::cstr16!(\"{}\"),\n", entry.args.escape_debug()).unwrap();
+        writeln!(dest_file, "        expand_boot_prefix: {},", entry.args.contains("${{PREFIX}}")).unwrap();
         write!(dest_file, "    }},\n").unwrap();
     }
     write!(dest_file, "]\n").unwrap();
```

The owned expanded argument buffer remains in scope through `start_image`.
The existing static strings are used unchanged for entries without the
token. The generated `expand_boot_prefix` field is an implementation detail,
not a JSON option. Runtime-directory-derived arguments still require owner
approval.

### A.2 1ES image build

Related design: [slot-owned ESP layout](#slot-owned-esp-layout).
The current image build sees the ESP mounted at `/boot`; consequently
`/boot/EFI/BOOT` becomes `EFI/BOOT` in the COSI ESP filesystem. Keep UKIs
in their existing `EFI/Linux` location. This diff changes the producer,
not the COSI schema or UKI/addon generation.

```diff
diff --git a/1es/acl/by-virt/mshv.yaml b/1es/acl/by-virt/mshv.yaml
index ed2befd..5be2fdb 100644
--- a/1es/acl/by-virt/mshv.yaml
+++ b/1es/acl/by-virt/mshv.yaml
@@ -37,3 +37,19 @@ config:

         mv /boot/efi/BOOT/grubx64.efi /boot/efi/BOOT/second.efi
         mv /boot/efi/efitreeloader.efi /boot/efi/BOOT/grubx64.efi
+
+        # The ESP is mounted at /boot in this image build.
+        bundle=/boot/EFI/BOOT
+        install -d "$bundle/Windows/System32"
+        for name in bootx64.efi grubx64.efi second.efi; do
+          if [ "/boot/efi/BOOT/$name" -ef "$bundle/$name" ]; then
+            continue
+          fi
+          install -m 0644 "/boot/efi/BOOT/$name" "$bundle/$name"
+        done
+        for name in HvLoader.efi lxhvloader.dll; do
+          install -m 0644 "/boot/efi/$name" "$bundle/$name"
+        done
+        for name in hvix64.exe hvax64.exe kdstub.dll; do
+          install -m 0644 "/boot/efi/Windows/System32/$name" "$bundle/Windows/System32/$name"
+        done
diff --git a/tools/build-efitreeloader.sh b/tools/build-efitreeloader.sh
index 51ee8af..3404e42 100755
--- a/tools/build-efitreeloader.sh
+++ b/tools/build-efitreeloader.sh
@@ -10,16 +10,15 @@ echo "Writing load_entries.json"

 # Write EFI load entries config file for efitreeloader build.
 #
-# Note: Currently ACL-T places the ESP at /boot instead of /boot/efi. So, all the paths get messed up. Also, neither
-# HvLoader.efi nor lxhvloader.dll support relative paths (which is fun). Hence why the extra \efi prefix exists.
+# Resolve DLL and hypervisor-root arguments against the executing loader's directory.
 #
 # Note: second.efi is the path that systemd-boot (grubx64.efi) gets moved to in the IC config (mshv.yaml).
-cat > "$EFITREELOADER_DIR/efitreeloader/load_entries.json" <<EOF
+cat > "$EFITREELOADER_DIR/efitreeloader/load_entries.json" <<'EOF'
 {
     "load_entries": [
         {
-            "path": "\\\\efi\\\\HvLoader.efi",
-            "args": "\\\\efi\\\\lxhvloader.dll MSHV_ROOT=\\\\\\\\efi\\\\Windows MSHV_ENABLE=TRUE MSHV_SCHEDULER_TYPE=ROOT MSHV_X2APIC_POLICY=ENABLE MSHV_SEV_SNP=TRUE MSHV_LOAD_OPTION=INCLUDETRACEMETADATA=1"
+            "path": "HvLoader.efi",
+            "args": "${{PREFIX}}\\lxhvloader.dll MSHV_ROOT=\\${{PREFIX}}\\Windows MSHV_ENABLE=TRUE MSHV_SCHEDULER_TYPE=ROOT MSHV_X2APIC_POLICY=ENABLE MSHV_SEV_SNP=TRUE MSHV_LOAD_OPTION=INCLUDETRACEMETADATA=1"
         },
         {
             "path": "second.efi"
```

The quoted heredoc preserves the literal `${{PREFIX}}` token and JSON
backslashes; neither shell expansion nor firmware-variable reads happen
during the build. The same-file check accommodates case-insensitive ESP
paths where `efi/BOOT` and `EFI/BOOT` already name the same directory.
The package source paths above follow the inspected
image recipe and must be confirmed against the exact release packages
during image validation. The existing signing pipeline must sign the
updated loader before release.

### A.3 Trident payload deployment and fallback

Related design: [deployment](#deployment) and [UEFI fallback](#uefi-fallback).
Keep the normal UKI, addon and boot-variable paths. Validate the entire
source bundle before the existing target-directory clearing step. The
new fallback helper touches only fixed allowlisted file names and their
`.new`/`.old` variants; it does not recursively delete directories.

```diff
diff --git a/crates/trident/src/subsystems/esp.rs b/crates/trident/src/subsystems/esp.rs
index a47c2cbb..eee52607 100644
--- a/crates/trident/src/subsystems/esp.rs
+++ b/crates/trident/src/subsystems/esp.rs
@@ -22,9 +22,9 @@ use osutils::{
 use trident_api::{
     config::UefiFallbackMode,
     constants::{
-        internal_params::DISABLE_GRUB_NOPREFIX_CHECK, EFI_DEFAULT_BIN_DIRECTORY,
-        EFI_DEFAULT_BIN_RELATIVE_PATH, ESP_EFI_DIRECTORY, GRUB2_CONFIG_FILENAME,
-        GRUB2_CONFIG_RELATIVE_PATH,
+        internal_params::{DISABLE_GRUB_NOPREFIX_CHECK, MSHV_SLOT_ESP_PAYLOAD},
+        EFI_DEFAULT_BIN_DIRECTORY, EFI_DEFAULT_BIN_RELATIVE_PATH, ESP_EFI_DIRECTORY,
+        GRUB2_CONFIG_FILENAME, GRUB2_CONFIG_RELATIVE_PATH,
     },
     error::{InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt},
     status::{AbVolumeSelection, ServicingState, ServicingType},
@@ -42,6 +42,8 @@ use crate::{
     },
 };

+mod mshv;
+
 #[derive(Default, Debug)]
 pub struct EspSubsystem;
 impl Subsystem for EspSubsystem {
@@ -54,6 +56,11 @@ impl Subsystem for EspSubsystem {
         // Perform file-based deployment of ESP images, if needed, after filesystems have been
         // mounted and initialized.

+        if ctx.is_stream_image && ctx.spec.internal_params.get_flag(MSHV_SLOT_ESP_PAYLOAD) {
+            return Err(TridentError::new(ServicingError::DeployESPImages))
+                .message("Slot-owned MSHV requires file-based ESP deployment");
+        }
+
         if !ctx.is_stream_image {
             deploy_esp(ctx, mount_path)?;
         }
@@ -91,6 +98,7 @@ pub fn set_uefi_fallback_contents(
             &source_esp_dir_name,
             &ctx.servicing_type,
             ctx.install_index,
+            ctx.spec.internal_params.get_flag(MSHV_SLOT_ESP_PAYLOAD),
         )?;
     } else {
         debug!(
@@ -352,11 +360,26 @@ fn copy_file_artifacts(

     // Generate list of filepaths to the boot files. Pass in the temp dir path where the image is
     // mounted to as an argument
-    let boot_files = generate_boot_filepaths(
-        temp_mount_dir,
-        ctx.is_uki().unstructured("UKI setting unknown")?,
-    )
-    .context("Failed to generate boot filepaths")?;
+    let mshv_payload = ctx.spec.internal_params.get_flag(MSHV_SLOT_ESP_PAYLOAD);
+    let source_bundle = temp_mount_dir.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
+    let boot_files = if mshv_payload {
+        ensure!(
+            cfg!(target_arch = "x86_64") && ctx.image_distro().is_acl(),
+            "Slot-owned MSHV requires x86-64 ACL"
+        );
+        ensure!(
+            ctx.is_uki().unstructured("UKI setting unknown")?,
+            "Slot-owned MSHV requires a UKI image"
+        );
+        mshv::validate_bundle(&source_bundle)?;
+        Vec::new()
+    } else {
+        generate_boot_filepaths(
+            temp_mount_dir,
+            ctx.is_uki().unstructured("UKI setting unknown")?,
+        )
+        .context("Failed to generate boot filepaths")?
+    };

     // Clear esp_dir_path if it exists
     if esp_dir_path.exists() {
@@ -375,12 +398,16 @@ fn copy_file_artifacts(
     }

     // Call helper func to copy boot files from temp_mount_dir to esp_dir_path
-    let grub_noprefix =
+    let grub_noprefix = if mshv_payload {
+        mshv::copy_bundle(&source_bundle, &esp_dir_path)?;
+        false
+    } else {
         copy_boot_files(temp_mount_dir, &esp_dir_path, boot_files).context(format!(
             "Failed to copy boot files from directory {} to directory {}",
             temp_mount_dir.display(),
             esp_dir_path.display()
-        ))?;
+        ))?
+    };

     if ctx.is_uki().unstructured("UKI setting unknown")? {
         // Prepare ESP directory structure for UKI boot
@@ -551,6 +578,7 @@ fn copy_boot_files_for_uefi_fallback(
     source_esp_name: &str,
     servicing_type: &ServicingType,
     install_index: usize,
+    mshv_payload: bool,
 ) -> Result<(), Error> {
     let esp_dir_path = path::join_relative(mount_point, esp_mount_path).join(ESP_EFI_DIRECTORY);
     if skip_uefi_fallback_copy(
@@ -574,7 +602,12 @@ fn copy_boot_files_for_uefi_fallback(
         source_esp_dir_path.display(),
         uefi_fallback_path.display()
     );
-    replace_boot_files(&source_esp_dir_path, &uefi_fallback_path).context(format!(
+    let result = if mshv_payload {
+        mshv::replace_bundle(&source_esp_dir_path, &uefi_fallback_path)
+    } else {
+        replace_boot_files(&source_esp_dir_path, &uefi_fallback_path)
+    };
+    result.context(format!(
         "Failed to copy boot files from directory '{}' to directory '{}'",
         source_esp_dir_path.display(),
         uefi_fallback_path.display()
diff --git a/crates/trident/src/subsystems/esp/mshv.rs b/crates/trident/src/subsystems/esp/mshv.rs
new file mode 100644
index 00000000..cf4494b5
--- /dev/null
+++ b/crates/trident/src/subsystems/esp/mshv.rs
@@ -0,0 +1,111 @@
+use std::{fs, path::Path};
+
+use anyhow::{ensure, Context, Result};
+
+const FILES: &[&str] = &[
+    "bootx64.efi",
+    "grubx64.efi",
+    "second.efi",
+    "HvLoader.efi",
+    "lxhvloader.dll",
+    "Windows/System32/hvix64.exe",
+    "Windows/System32/hvax64.exe",
+    "Windows/System32/kdstub.dll",
+];
+
+fn validate_tree(root: &Path, directory: &Path, temporary: bool) -> Result<()> {
+    ensure!(
+        fs::symlink_metadata(directory)?.is_dir(),
+        "Invalid payload directory"
+    );
+    for entry in fs::read_dir(directory)? {
+        let entry = entry?;
+        let path = entry.path();
+        let relative = path.strip_prefix(root)?;
+        let kind = entry.file_type()?;
+        if kind.is_dir() {
+            ensure!(
+                relative == Path::new("Windows") || relative == Path::new("Windows/System32"),
+                "Unexpected payload directory: {}",
+                relative.display()
+            );
+            validate_tree(root, &path, temporary)?;
+        } else {
+            ensure!(kind.is_file(), "Payload entry is not a regular file");
+            ensure!(
+                FILES.iter().any(|name| {
+                    relative == Path::new(name)
+                        || temporary
+                            && (relative == Path::new(&format!("{name}.new"))
+                                || relative == Path::new(&format!("{name}.old")))
+                }),
+                "Unexpected payload file: {}",
+                relative.display()
+            );
+        }
+    }
+    Ok(())
+}
+
+pub(super) fn validate_bundle(root: &Path) -> Result<()> {
+    validate_tree(root, root, false)?;
+    for name in FILES {
+        ensure!(
+            fs::symlink_metadata(root.join(name))
+                .with_context(|| format!("Missing MSHV payload file: {name}"))?
+                .is_file(),
+            "Invalid MSHV payload file: {name}"
+        );
+    }
+    Ok(())
+}
+
+pub(super) fn copy_bundle(from: &Path, to: &Path) -> Result<()> {
+    validate_bundle(from)?;
+    for name in FILES {
+        let destination = to.join(name);
+        fs::create_dir_all(destination.parent().context("Missing payload parent")?)?;
+        fs::copy(from.join(name), destination)?;
+    }
+    Ok(())
+}
+
+pub(super) fn replace_bundle(from: &Path, to: &Path) -> Result<()> {
+    validate_bundle(from)?;
+    if fs::symlink_metadata(to).is_ok() {
+        validate_tree(to, to, true)?;
+        ensure!(
+            fs::canonicalize(from)? != fs::canonicalize(to)?,
+            "Source and fallback directories must differ"
+        );
+    }
+    for name in FILES {
+        let destination = to.join(name);
+        let previous = to.join(format!("{name}.old"));
+        fs::create_dir_all(destination.parent().context("Missing payload parent")?)?;
+        if previous.exists() {
+            if destination.exists() {
+                fs::remove_file(&previous)?;
+            } else {
+                fs::rename(&previous, &destination)?;
+            }
+        }
+        fs::copy(from.join(name), to.join(format!("{name}.new")))?;
+    }
+    for name in FILES {
+        let destination = to.join(name);
+        if destination.exists() {
+            fs::rename(&destination, to.join(format!("{name}.old")))?;
+        }
+    }
+    for name in FILES {
+        fs::rename(to.join(format!("{name}.new")), to.join(name))?;
+    }
+    for name in FILES {
+        let previous = to.join(format!("{name}.old"));
+        if previous.exists() {
+            fs::remove_file(previous)?;
+        }
+    }
+    Ok(())
+}
diff --git a/crates/trident_api/src/constants.rs b/crates/trident_api/src/constants.rs
index 93c089b4..9f635f41 100644
--- a/crates/trident_api/src/constants.rs
+++ b/crates/trident_api/src/constants.rs
@@ -191,6 +191,9 @@ pub const VALID_CONFEXT_DIRECTORIES: [&str; 3] = [

 /// Internal-only overrides
 pub mod internal_params {
+    /// Enable the experimental slot-owned MSHV ESP payload.
+    pub const MSHV_SLOT_ESP_PAYLOAD: &str = "mshvSlotEspPayload";
+
     /// Allow unused images in a COSI file.
     pub const ALLOW_UNUSED_FILESYSTEMS_IN_COSI: &str = "allowUnusedFilesystems";

```

### A.4 Remaining integration work

These diffs deliberately do not invent implementations for unresolved
contracts. Before enabling the internal parameter in a release:

- Define and validate reliable running-hypervisor identity evidence
  (mechanism TBD) without creating a temporary UEFI variable. The resolver
  diffs alone do not provide this evidence.
- In `crates/trident/src/engine/rollback.rs`, extend `validate_boot`'s
  existing root/slot check with the applicable MSHV hypervisor/OS comparison,
  before user health hooks and any boot-default/fallback promotion.
  Do not implement it as a new check in `run_health_checks` or
  `health::execute_health_checks`. Return
  `ServicingError::ValidateAbActiveHypervisor` following the existing
  `ValidateAbActiveVolume` validation-error pattern. To implement the
  [auto-rollback scaffolding](#scaffolding-for-invalid-hypervisor-auto-rollback),
  update failure handling to explicitly persist that error and the
  proposed `AbUpdateHypervisorCheckFailed` state for update hypervisor
  failures, then initiate rollback through explicitly extended result/error
  handling. Preserve existing root-slot mismatch behavior and reserve
  `AbUpdateHealthCheckFailed` for actual health-check failures. Validate the
  retained OS/hypervisor pair before completing recovery from the new
  state. Apply the expanded slot check to manual rollback and recovery
  branches that mark the old OS provisioned without running user health
  hooks.
- In `crates/trident_api/src/status.rs` and `crates/trident_api/src/error.rs`,
  add the new persisted failure state and
  `ServicingError::ValidateAbActiveHypervisor`. Update serialization/status
  API mappings and state consumers in
  `crates/trident/src/lib.rs`, daemon datastore handling and manual rollback
  dispatch. Commit must recognize the new state, request the appropriate
  rollback/reboot, select the retained slot for recovery and preserve the
  original error. Do not reuse a health-check result variant to classify
  the hypervisor failure.
- In `crates/trident/src/engine/boot/uki.rs`, reuse the existing slot/install
  naming rules for expected UKI identity. Do not equate an ESP directory
  name with a UKI filename suffix for higher installation indices.
- In `crates/trident/src/subsystems/esp.rs`, add peak-allocation preflight,
  reject conflicting legacy/unsupported transitions before target clearing,
  and finalize persisted opt-in/opt-out handling. A payload's presence alone
  is not permission to bypass the expanded MSHV slot validation.
- In the 1ES build/release pipeline, enable the internal parameter only
  after signing, COSI inspection, DLL relocation, nested fallback, automatic
  rollback and firmware/VM acceptance pass. Do not enable TPM/PCR-bound
  encryption until its separate event-prediction/unlock tests pass.

No temporary UEFI variable, metadata marker, provenance file or payload-hash
contract is introduced by the proposed diffs.

### A.5 Validation of the proposed diffs

The appendix patches were checked against the pinned source baselines in
isolated Linux worktrees. Test implementations used for validation are
omitted from the examples. Validation covers:

- Efitreeloader: 38 host unit tests, including executing-directory prefix
  normalization and rejection of unsupported or malformed directories;
  generated-configuration checks for exact-token detection and literal
  compatibility; release-mode `cargo check` for `x86_64-unknown-uefi`;
  Rust formatting.
- Trident payload helper: five filesystem unit tests in an isolated
  harness, including incomplete payloads, interrupted replacement and
  symlink rejection. This is not a full Trident workspace build.
- Image build: shell syntax, YAML parsing, literal JSON template
  generation and payload assembly with separate and aliased source paths.
- RFC: patch applicability, JSON examples, section links, MDX compilation
  and whitespace checks.

No signed image, COSI production run, firmware boot or VM rollback test
has been performed. The integration work in [A.4](#a4-remaining-integration-work)
is still required.
