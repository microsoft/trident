# 0924 Slot-Owned MSHV Boot Payloads

- Date: 2026-11-05
- RFC PR: [microsoft/trident#924](https://github.com/microsoft/trident/pull/924)
- Issue: Not created.
- Status: Draft proposal; not implemented or boot-validated.

## Summary

Enable Trident to install, update and roll back MSHV-enabled Azure Container
Linux (ACL) images by keeping a complete hypervisor boot payload beside each
slot's EFI chain. Preserve existing firmware and UKI selection. Add mandatory
boot-identity validation before commit so a mixed hypervisor/OS boot cannot
be promoted. Two alternative efitreeloader changes are proposed: select
complete signed argument profiles by directory, or expand a signed argument
template using the current firmware boot entry. The resolver choice and its
signing-policy approval remain open.

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

Efitreeloader's JSON configuration is compiled into its signed binary.
The loader already resolves relative child executable paths against its
executing directory using `LoadedImageDevicePath`. However, the current
HvLoader/DLL arguments name fixed absolute DLL and hypervisor-root paths.
Moving only the EFI chain therefore does not make the entire payload
slot-owned.

HvLoader verifies and loads the external DLL. Its public source describes
preparing the hypervisor without running it and registering a kernel-facing
protocol. This RFC does not assume that it starts an independent VM before
booting Linux, or that its public source establishes the external DLL's
complete verification and measurement behavior.

[microsoft/trident#915](https://github.com/microsoft/trident/pull/915)
proposes preserving the existing shared-path payload during clean install.
That is useful initial enablement, but it does not provide slot-owned
payload servicing or retain separate hypervisor versions for rollback.

### Problem

An A/B update must not overwrite the committed slot's hypervisor files.
Otherwise, reverting the OS can leave it using the updated release's
hypervisor. The ESP is shared storage, so this requires explicit ownership
and copying, not a new A/B partition.

There are also two independent boot selectors:

| Layer | Selection |
| --- | --- |
| Firmware | `BootNext`/`BootOrder` select a `Boot####` entry containing an ESP device path and a path such as `\EFI\AZLB\bootx64.efi`. |
| Systemd-boot | `LoaderEntryOneShot`/`LoaderEntryDefault` select a UKI in shared `EFI/Linux`. `LoaderEntrySelected` reports the selected entry. |

These selectors can diverge. For example, an A-to-B trial selects B's EFI
chain and B's UKI. If B fails before systemd-boot consumes the UKI oneshot,
firmware may try A's chain, prepare A's hypervisor, then start B's UKI.
Slot-owned payloads introduce a compatibility consequence to this existing
selector divergence.

The proposal preserves those selectors. Its guarantee is **no successful
promotion of an observed mixed boot**, not prevention of every transient
mixed launch.

### Goals

- Service the hypervisor payload with the OS using the existing image and
  Trident lifecycle.
- Retain the committed payload unchanged while staging an update, and reuse
  it for automatic and manual rollback.
- Keep efitreeloader out of UKI selection.
- Detect inconsistent preparation/UKI/root identity before default or
  fallback promotion.
- Preserve existing behavior for non-MSHV and legacy images.

## Scope

### Requirements

- Initially support opt-in x86-64 ACL UKI images deployed through the normal
  file-based ESP path.
- Package a complete relocatable source bundle; reject incomplete or
  unsupported bundles before clearing the target.
- Stage only the target slot and keep the committed slot bootable.
- Preserve Trident's UKI naming, addon handling, firmware registration,
  servicing states and failure propagation.
- Copy the complete payload for configured UEFI fallback, including nested
  files and payload-origin metadata.
- Require fresh preparation evidence and identity validation before commit,
  manual-rollback success and rollback-recovery success.
- Preserve executable verification and obtain approval for the selected
  child-argument policy.
- Preflight actual peak ESP use without deleting the committed slot.

### Out of Scope

- A new active-slot EFI variable, loader-driven UKI pinning, or a second
  servicing state machine.
- Arbitrary shared-ESP file updates or non-UKI MSHV deployment.
- Raw-image/disk-streaming adoption and conversion of already installed
  shared-path MSHV layouts in the initial release.
- Guaranteed recovery from every pre-Linux hang or arbitrary power loss.
- Authenticated release pairing, runtime attestation and new anti-rollback
  policy. Diagnostic evidence is not a security authority.
- A new UKI-prerun facility or relative-path changes inside HvLoader/DLL.

### Exit Criteria

Clean install, A/B update, commit, failed-trial recovery and manual rollback
are demonstrated with distinguishable A/B payloads and matching UKIs/root
devices. A mixed boot reaching Linux fails before any promotion. Unsupported
transitions, invalid evidence and insufficient space fail explicitly.
Secure Boot, actual DLL relocation, configured fallback and supported
physical-firmware behavior are validated. TPM/PCR-bound encryption remains
rejected until its additional measurement and unlock coverage passes.

## Dependencies

- **Efitreeloader:** implement one resolver alternative below and the scoped
  preparation-record protocol. Its owner requires signed child arguments;
  neither selection-policy approval nor template-expansion approval is
  assumed.
- **HvLoader and external loader DLL:** demonstrate that ESP-absolute DLL
  and root paths work in each slot and fallback directory, without silently
  substituting fixed shared payloads. Complete DLL source behavior has not
  been established.
- **Image producer:** build and sign the loader, assemble the marked
  relocatable ESP bundle, and deliver it through the image's existing ESP
  content. No COSI schema revision is proposed.
- **Trident:** extend ESP copying and mandatory health validation.
  Coordinate with microsoft/trident#915's pending legacy clean-install
  behavior; do not assume that PR already enables this layout.

## Implementation

### Slot-owned ESP layout

The source image supplies one canonical bundle under `EFI/BOOT`. Trident
copies it into the target slot directory and writes managed origin metadata.

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
      mshv-layout
      mshv-origin
    AZLB/
      ...independently versioned bundle...
    BOOT/
      ...configured fallback copy, including its payload origin...
    Linux/
      vmlinuz-100-azla0.efi
      vmlinuz-100-azla0.efi.extra.d/
      vmlinuz-101-azlb0.efi
      vmlinuz-101-azlb0.efi.extra.d/
```

The marker contains the exact ASCII bytes `slot-v1\n`. Validate this version
and the complete allowlisted tree as regular files/directories, rejecting
links, special files and unexpected content. The only payload directory
tree is `Windows/System32`.

UKIs remain in shared `EFI/Linux`; their existing `.extra.d` staging and
ACL slot/firstboot policies remain unchanged. `AZL2B` is an additional
installation's EFI directory and corresponds to UKI suffix `azlb1`, not
`azlb2`. Reuse Trident's naming helpers.

### Efitreeloader resolver alternatives

Choose **one** of these mechanisms. Both keep relative child executable
resolution and pass concrete ESP-absolute DLL/root arguments to HvLoader.
Neither writes `LoaderEntry*` or selects a UKI. Neither requires HvLoader or
the DLL to implement relative-path support.

#### Alternative A: signed directory profiles

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

Discover the executing parent with existing `LoadedImageDevicePath`
handling. Match exactly one canonical root, case-insensitively for the
initial ASCII directory names, and execute its entries in order. Pass
arguments unchanged. The mechanism must not understand ACL or A/B names.

Reject duplicate/malformed roots, empty profiles and documents mixing the
legacy and profile forms at build time. Reject zero or multiple runtime
matches before starting any child. Retain the existing top-level
`load_entries` schema as a separate legacy configuration.

This bounds runtime selection to complete signed strings. Additional roots
such as `AZL2A`/`AZL2B` require explicit profiles and release coverage.
Reject unsupported install indices before deployment; an unsigned manifest
must not expand the signed root list.

#### Alternative B: runtime boot-entry prefix

Compile one argument template into the signed efitreeloader. Efitreeloader,
not HvLoader or the DLL, expands the designated `${{PREFIX}}` token before
invoking the child.

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
BootCurrent = 0008
  -> Boot0008 = ESP device path + \EFI\AZLB\bootx64.efi
  -> ${{PREFIX}} = \EFI\AZLB
  -> DLL argument = \EFI\AZLB\lxhvloader.dll
     MSHV_ROOT=\\EFI\AZLB\Windows
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

This avoids enumerating each install directory, but **signing a template
does not sign the prefix read from mutable firmware state**. Explicit
owner/security-policy approval of the substitution and validation contract
is required. It is not an approved relaxation of the signed-argument
requirement.

For both alternatives, preserve the current extra-leading-backslash
`MSHV_ROOT` convention and validate it with the packaged DLL. Keep argument
storage alive through child execution and preserve its encoding.

### Deployment and payload identity

Introduce a separately gated slot layout, provisionally
`internalParams.mshvSlotEspPayload`. Require both this opt-in and the exact
source marker. Reject non-UKI images, incomplete bundles, conflicting legacy
flags and unsupported resolver coverage before modifying the target.

Use the existing target-directory calculation, inactive-slot clearing,
deployment, flush/unmount and finalization paths. Replace only the
boot-file copy operation with complete-bundle copying. Do not copy updated
files into the committed slot or the legacy fixed `\efi` paths.

`mshv-origin` records `kind`, managed `install-index`, `slot` and
`payload-id`. It contains no UKI filename or boot preference. The copied
origin remains with a fallback snapshot so BOOT's payload can be attributed
without a mutable global active-slot setting.

For `slot-v1`, define the diagnostic payload ID as SHA-256 over these
ordered files' final signed bytes:

```text
bootx64.efi
grubx64.efi
second.efi
HvLoader.efi
lxhvloader.dll
Windows/System32/hvix64.exe
Windows/System32/hvax64.exe
Windows/System32/kdstub.dll
```

For each UTF-8 relative name, hash its byte length as little-endian `u64`,
the name bytes, the file length as little-endian `u64`, and file bytes.
Exclude marker/origin metadata to avoid a self-referential digest. Builder
and consumer must agree. This identifies the deployed bundle; it is not
authorization or independent evidence of what the external DLL loaded.

Complete validation, copying and persistence before ordinary firmware/UKI
finalization advertises the trial. A failed copy uses existing deployment
errors and retry semantics. No new payload-generation state machine or
second boot-variable finalizer is introduced.

### Mandatory boot-identity validation

For the new MSHV sequence only, efitreeloader preflights the bundle/origin
and publishes a bounded, versioned preparation record. Write
`prepared=false` before possible HvLoader preparation, then overwrite with
`prepared=true` only after successful return. An error or partial
preparation must never leave a successful current-attempt record.

Proposed private protocol:

| Field | Contract |
| --- | --- |
| Variable | `MshvBootPreparation` |
| Vendor GUID | `d3f18c74-6ab2-4e13-9d6a-79828a5239e1` (proposed, not allocated by this RFC) |
| Attributes | Boot-service and runtime access; **not** nonvolatile |
| Encoding | Bounded UTF-8 fields: `version`, `image-dir`, `prepared`, plus origin fields |
| Parsing | Reject missing/duplicate fields, unknown versions, malformed values and control-character injection. |

Gate record handling to the new MSHV sequence; do not change unrelated
efitreeloader entries. Verify reset/freshness behavior on supported firmware,
including warm reset. A saved disk record is never current-boot evidence.
Record-write failures before or after preparation must not allow promotion.

Trident compares the preparation record, `LoaderEntrySelected`, existing
root/verity validation and the expected servicing target. Use `BootCurrent`
as corroboration, not an arbitrary label as proof of the executed chain.
An AZLA preparation with a B UKI/root is a mismatch. A BOOT payload copied
from A is also a mismatch with B even though its executing directory is BOOT.

Run this mandatory check before user health hooks and before `BootOrder`,
`LoaderEntryDefault` or fallback promotion. Reuse existing health-error
handling: an update enters `AbUpdateHealthCheckFailed`; clean install
reports provisioning failure. Cover manual rollback and automatic
rollback-recovery success even where existing user health hooks do not run.
Missing or contradictory evidence must not declare the machine provisioned.

Existing firmware promotion uses the label of `BootCurrent`. An explicit
BOOT entry therefore needs deliberate mapping to the validated managed
entry. Until that is defined and tested, reject ambiguous promotion rather
than accidentally promoting BOOT.

The record attests only operationally to the sequence whose preparation
returned success. Privileged writers can spoof origin metadata and EFI
variables. Use a trustworthy runtime hypervisor identity query if available;
do not equate this diagnostic protocol with cryptographic attestation.

### Lifecycle

| Phase | Behavior |
| --- | --- |
| Clean install | Stage A's complete bundle and UKI with existing ACL verity/firstboot handling. Register/finalize using existing selectors; do not invent a usable B chain. Validate identity before success. |
| A-to-B update stage | Stage only B's OS, UKI/addons and complete payload. Preserve A and strip the image firstboot addon as today. |
| Update finalize | Complete payload publication before setting existing B firmware/UKI trial selectors. Keep committed A defaults and existing fallback-mode decisions. |
| Trial and commit | Require matching preparation/UKI/root identity and health before promoting firmware default, UKI default or fallback. |
| Failed trial | Reuse existing health rollback. With B's oneshot consumed and committed A defaults intact, the next reset should restore A+A; verify those conditions. |
| Manual rollback | Select retained A using existing firmware/UKI logic. Validate A independently of whether a new UKI is staged, and require matching identity before success. |
| Later slot reuse | Replace the old inactive bundle only after the other slot is committed. Never refresh the rollback payload from the newer image. |

A boot that hangs before Linux cannot be observed by commit. Reuse existing
firmware fallback and platform watchdog/external reset behavior. Reset does
not itself clear an unconsumed `LoaderEntryOneShot`; do not promise A+A
recovery after every pre-systemd failure. Repeated mismatch must report
failure rather than oscillate slots. Add a narrow recovery change only if
testing demonstrates a new MSHV-specific failure.

### UEFI fallback

Copy the chosen slot's complete bundle, including `Windows/System32` and
origin metadata, to `EFI/BOOT`. Resolve runtime paths to BOOT's own files.
This is a payload snapshot, **not** a self-contained fallback OS: UKIs stay
shared, and no copied exact-UKI selector is added.

Preserve existing fallback modes:

| Mode | During B trial | After successful B commit |
| --- | --- | --- |
| Conservative | Retain A payload in BOOT. | Copy B payload. |
| Optimistic | Copy B payload to BOOT. | Retain B payload. |
| Disabled | Do not create/update fallback. | Unchanged. |

The existing flat `replace_boot_files` copy cannot copy nested directories.
Extend its copy/removal operations only for validated marked bundles,
retaining its existing `.new`/`.old` phases, callers, return type and
servicing-error propagation. Stage the complete replacement tree before
renaming; handle only exact, validated Trident-owned stale entries on retry.
Never recursively remove an ESP root, arbitrary directory or active slot.

An MSHV fallback-copy failure has the same state/error behavior as ordinary
fallback-copy failure. The extension does not make FAT publication an atomic
whole-bundle transaction or expand existing power-loss guarantees.

Exercise optimistic B fallback surviving a failed trial and later B restage.
A mixed fallback boot reaching Linux must fail validation; an ambiguous
BOOT-to-managed promotion remains unsupported.

## Public API Design

No new public Host Configuration API or COSI metadata revision is proposed.
The provisional internal flag is off by default and requires the versioned
source marker. Its final name and supported combinations are implementation
review items, not an existing API.

Efitreeloader configuration changes are described above. Alternative A adds
a strict `profiles` form while retaining legacy `load_entries`. Alternative
B adds explicitly gated template interpretation; legacy literals and
non-MSHV behavior must remain unchanged.

The marker, origin metadata and preparation record form a versioned
producer/loader/Trident contract. Resolve final protocol ownership and
versioning before enabling production images.

## Testing and Metrics

### Focused tests

| Area | Coverage |
| --- | --- |
| Profiles (A) | Legacy compatibility, strict schema, duplicate/malformed roots, zero/multiple matches, case handling, ordering and unchanged argument bytes; unlisted roots fail before child execution. |
| Prefix (B) | Binary load-option/device-path parsing, UTF-16 paths, device identity, exact substitution, argument lifetime/encoding, malformed/missing variables, multiple ESPs, and executing-path mismatch. |
| Payload deployment | Complete allowlist, links/special-file rejection, digest agreement, inactive-only writes, slot reuse, firstboot/addon preservation, insufficient space, failed-copy retries and non-MSHV compatibility. |
| Identity guard | A+B and B+A mismatch, higher install-index naming, malformed/incomplete/stale records, BOOT origin and ambiguous promotion; verify errors reach existing health-failure state/reboot handling. |
| Fallback | Nested files and metadata; failures during copy/rename/cleanup; stale exact `.new`/`.old` entries; same servicing errors as ordinary fallback failures and no active-slot mutation. |

### VM and firmware acceptance

| Scenario | Required evidence |
| --- | --- |
| Install A, update B, commit B | Distinct A/B payload hashes, matching UKI/root and preparation record; A unchanged during B trial. |
| B EFI failure before oneshot consumption | Inject A+B launch; if Linux starts, fail before promotion and verify A+A after consumed-oneshot recovery with intact defaults. |
| Failure after oneshot consumption; health failure | Verify retained payload, existing selector recovery and no stale successful record. |
| Pre-Linux hang/failure | Exercise firmware/watchdog/external reset; document unconsumed-oneshot behavior without claiming OS health checks ran. |
| B commit followed by manual rollback A | Retained A files and validated A preparation/UKI/root before success. |
| Conflicting defaults, menu selection, chainloading | Reject mixed identity or unsupported resolver attribution. |
| Entry loss or explicit BOOT entry | Correct payload-origin attribution and fail-closed ambiguous promotion. |
| Fallback modes and repeated slot reuse | Conservative/optimistic/disabled behavior; failed-B fallback followed by restaging does not bypass validation. |
| Copy/publication/finalization interruption | Compare existing failure/retry behavior, preserve committed payload and do not advance servicing state incorrectly. |
| Secure Boot | Approved signed loader, signer/SBAT policy and retained downstream verification; exact packaged DLL loads the intended Intel/AMD payload. |
| Measurement and encryption | Capture actual event log and validate prediction/unlock before allowing TPM/PCR-bound encryption. |
| Legacy or unsupported images | Preserve existing paths, or reject unsupported conversion before modifying the target. |

Run Trident `functional-test` cases only inside VMs, never on a bare build
host. OVMF results alone do not establish supported physical-firmware
behavior. Inspect the built image/COSI and run the image workspace's
existing validation before boot testing.

Use existing structured errors and health results to report preparation
failure, identity mismatch, unsupported layout/resolver and capacity
failure. Capture selected UKI, origin and firmware entry in diagnostic
logs. No new telemetry service or attestation metric is proposed.

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
metadata and temporary replacement copies.

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
later requires full-tree bootstrap, source-to-managed origin conversion
and independently tested rollback/fallback behavior.

An image must not be offered for this layout to an older consumer that
does not understand its marker, preparation record or mandatory guard.
Establish minimum producer/loader/Trident versions in the release pipeline;
do not claim the marker alone makes older binaries reject the image.
Removing the opt-in must not bypass validation of an already managed MSHV
boot; define persisted layout detection and reject unsafe disablement.

## Implementation Plan

1. Resolve the signing-policy/resolver choice and demonstrate packaged DLL
   relocation for A, B and BOOT. Stop if either prerequisite fails.
2. Implement the chosen generic loader resolver, image bundle and scoped
   preparation protocol. Verify exact arguments and actual preparation.
3. Implement mandatory pre-promotion validation and existing health-failure
   routing, including mixed trial, manual rollback and recovery.
4. Extend inactive-slot and fallback copying with existing state/error
   semantics; test interruption, retry, slot reuse and origin retention.
5. Complete Secure Boot, measurements, capacity and supported firmware
   validation, then enable the opt-in in production image pipelines.

## Counter-Arguments

### Drawbacks

Slot-owned payloads consume more shared ESP space and increase boot-chain
and image-producer coordination. The independent firmware/UKI selectors
still permit transient mixed launches; pre-Linux failures require platform
recovery. Diagnostic evidence cannot prove authenticity against privileged
writers.

Profiles require signed-build coverage for every allowed root. Prefix
expansion reduces that enumeration but depends on correct firmware-option
attribution and an explicitly approved runtime-argument policy.

### Alternatives

- **Keep one shared payload:** simpler, but an update overwrites the
  committed release's rollback payload.
- **Have efitreeloader pin a UKI:** adds another selector and changes
  existing ownership. Exact UKI references in copied fallback metadata can
  also outlive Trident's UKI cleanup. Not proposed.
- **Add a global active-slot variable:** can diverge from the executing
  firmware chain and adds state synchronization rather than attribution.
- **Require a new publication/reset state machine:** not justified by a
  demonstrated MSHV-specific regression. Extend existing copy and recovery
  behavior first and document its limits.
- **Prepare after UKI selection:** a possible long-term direction, but no
  existing UKI-prerun mechanism or interface has been identified. It is not
  a dependency of this short-term proposal.

## Open Questions

- Which resolver is accepted by the efitreeloader owner? Does the approved
  signed-argument policy permit prefix expansion, and with what validation?
- Does the packaged external DLL support each absolute root without
  substitution, and is a trustworthy runtime payload identity available?
- Who owns the private preparation protocol, final GUID, bounds and
  versioning? How is freshness established across supported reset paths?
- How should an explicit BOOT entry be mapped to a managed firmware entry
  for promotion? Until resolved, ambiguous promotion remains blocked.
- What persisted layout/identity state prevents unsafe opt-out and supports
  expected-payload diagnostics through commit and rollback?
- Which minimum component versions, install roots and firmware platforms
  are supported at launch? What measured peak ESP size do their images need?

## Future Possibilities

Support raw-image adoption, validated legacy conversion, more install roots
and TPM/PCR-bound encryption after their prerequisites pass. A separate
long-term design could prepare the hypervisor after UKI selection and add
relative-path support inside HvLoader and its DLL. No such facility is
assumed available by this RFC.

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
