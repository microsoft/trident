# 0838 Trident-Managed Extensions

- Date: 2026-10-15
- RFC PR: [microsoft/trident#838](https://github.com/microsoft/trident/pull/838)
- Status: Proposal.

## Summary

Provide an **A/B update path for sysexts on ACL**. Image builders bundle their
sysexts in the update COSI so Trident can stage, activate and roll them back
with the OS, rather than requiring a separate stack updater. An opt-in Host
Configuration policy controls storage and activation for sysexts and confexts,
while preserving shared user state.

## Motivation and Goals

### Background

Systemd **system extensions (sysexts)** supply `/usr` and `/opt` content;
**configuration extensions (confexts)** supply `/etc` content. Here they are
Discoverable Disk Images (DDIs): mountable filesystem images containing those
files and compatibility metadata.

A **Composable OS Image (COSI)** is a tar of OS partition images and metadata.
RFC 0789 proposes COSI 1.3 support for bundled extensions: optional metadata arrays
`extensions.sysexts` and `extensions.confexts` reference compressed DDIs by ID,
location, size and hash. OS and extensions can then ship as one artifact;
Trident can also fetch extensions listed separately in Host Configuration.
See the [example COSI cutaway](./0838-cosi-anatomy.svg).

#### ACL storage and update behavior

Azure Container Linux (ACL) already uses sysexts for containerd and OEM
integration and is expected to rely more on them.

Only the `/usr` filesystem is A/B-paired and dm-verity-protected, with paired
hash partitions; **all mutable filesystems are shared**. ESP, OEM and ROOT are
unpaired, and `/etc`, `/var` and `/opt` live on shared ROOT.

Images embedded in `/usr` follow its slot. For images on shared storage,
changing the stored files affects both slots; switching `/usr` does not undo
those changes.

### Problem Statement

Users build custom images on ACL, often adding stack files or preloaded
sysexts on mutable storage. ACL's A/B update replaces the core OS, not those
additions. Their stack therefore needs an out-of-band updater even though it
already ships in their image.

### Goals

- Update and roll back ACL's OS and declared sysexts together, using one COSI
  instead of an out-of-band stack updater.
- Activate the matching extension set before consumers run on every boot,
  including rollback.
- Preserve user state while confext defaults follow the OS slot.

## Scope

**ACL is the initial design and implementation target** because its sealed
A/B `/usr` and shared mutable storage require explicit slot selection.
Support for other platforms is future work. The public API avoids ACL-specific
settings so it can be reused later.

The mode selects how Trident consumes COSI-bundled extensions. This RFC
develops `managed` (alias `acl`) for ACL; `simple` and `native` describe direct
placement on writable A/B filesystems.

Explicit Host Configuration `os.sysexts` and `os.confexts` declarations remain
honored in every mode. They retain existing placement behavior except in
`managed`/`acl`, which owns the combined COSI and Host Configuration set.

### Requirements

- Select bundled-extension handling through one public mode.
- A published store must contain valid manifests for every bootable slot;
  images without a store need no initialization.
- Replace the inactive slot's managed extension set on reuse; do not retain
  older sets for that slot.
- Preserve each platform's filesystem mutability and user-state persistence.
- Support ACL's shared storage without new partitions or systemd source changes.
- Keep the policy reusable on platforms with writable A/B storage.

### Out of Scope

- Image Customizer's managed-content authoring/bundling API and build design
  (a required separate proposal; see [Dependencies](#dependencies)).
- Relocating the Trident-owned store; see [Future Possibilities](#future-possibilities).
- Updating arbitrary files on shared mutable storage.
- Runtime Update for managed extension/policy changes in the initial release.
- New image-less updates, initrd-scoped sysext content or ESP payload storage.
- Bundled disk streaming; 0789's exclusion remains.

### Exit Criteria

- A COSI file carrying extensions completes an ACL A/B update with the OS,
  sysexts and confexts staged for the same target slot.
- Update, commit, health-check rollback and bootloader fallback expose matching
  slot content before consumers run, without partial activation or late repair.
- Repeated slot reuse removes obsolete extension files without changing the
  active slot or accumulating older sets.
- The public API, safe adoption and tests below are delivered;
  user `/etc` edits and SELinux Enforcing are preserved.

## Dependencies

- **[RFC 0789](https://github.com/microsoft/trident/pull/789):** COSI extension
  metadata contract and reader/writer support.
- **Image Customizer (IC):** a separate, out-of-scope API and build design for
  declaring extensions, maintaining the store and manifests, and bundling
  extensions into COSIs. Required for the end-to-end image-building workflow;
  Trident accepts a conforming COSI from any producer.

## Implementation

### Design

This section describes `managed`/`acl` on ACL.

Trident keeps the old and new extension files side by side. An update prepares
the inactive slot's files without changing the running system. At boot,
`trident-slot-loader` selects the files for the OS slot that actually booted.
Rollback uses the same selection process.

![Managed extension delivery and activation on ACL](./0838-managed-extensions-overview.svg)

#### Where the files live

The [store](#managed-content-store) is fixed at `/var/lib/trident/slots`. Its `a/` and `b/` directories
hold each slot's sysexts and confexts on shared ROOT. These are separate
directories, not new partitions. If A is running, an update prepares B's set
while retaining A's files. **Reusing B replaces its entire managed extension
set**, just as staging a new OS replaces B's partition contents. Keep one set
per slot, not a history of sets within each slot.

The [staged update](#trident-staging) takes effect on reboot into the target OS slot.

#### Booting after an update

The **[Slot Loader Manifest](#slot-loader-manifest)** tells the loader exactly which managed extensions
belong to the booted slot.
It is created when the store is initialized, during
[image construction](#base-image-construction-and-ic-integration) or first
servicing, and replaced during update staging. No additional COSI format change
is required.

After reading `acl.slot`, the loader opens
`/sysroot/var/lib/trident/slots/<slot>/slot-loader.json` in the initrd.
It uses this file, not Trident's SQLite database or a directory scan.

The loader [returns success without activation](#loader-startup-behavior) when
the store is absent or a valid manifest selects no images.
A loader failure on an uncommitted update boot triggers
[diagnostics and reboot](#update-boot-failure-recovery) into the committed OS.

#### Writable `/etc`

User edits in the writable **upper** take precedence over [extension defaults](#confext-handling),
which take precedence over factory defaults. Changing OS slots changes the
extension defaults but keeps that shared upper, so user edits survive.
The native sysext merge still runs after `/etc` assembly.

### Component deep dives

#### Managed content store

The managed content store is persistent storage for Trident-managed content
associated with an OS slot. In this design, it holds each slot's sysext/confext
`.raw` images and Slot Loader Manifest (`slot-loader.json`). The initial ACL
implementation uses only `/var/lib/trident/slots`, with separate `a/` and `b/`
directories on shared ROOT.

The path must resolve to ACL's shared ROOT filesystem. ROOT must be mounted
before the loader runs and writable during staging. Target-slot selection
comes from the host's A/B configuration, not source COSI partition numbers.

#### Base Image Construction and IC Integration

ACL images may omit the managed content store entirely. **If a store is
present, it must contain valid manifests for every bootable slot**, even if
they select no images. `{"version": 1}` is a valid empty manifest. Boot must
not depend on Trident servicing or datastore initialization.

IC must support adding, replacing and removing managed payloads while
publishing their files and manifests together. Its API remains a
[separate proposal](#dependencies).

Integrating `trident offline-initialize` into ACL builds remains an independent
goal. A store/manifest initialization extension could let ACL scripts and IC
discover managed slot payloads in the image filesystem, validate them and write
manifests without requiring a servicing datastore or adopting unrelated
factory/operator extensions.

Any build-time initialization must finish before publishing the image. The
loader must not invoke initialization to repair a missing manifest.

#### Trident staging

This sequence applies to `managed`/`acl`. Direct-placement modes follow the
[mode policies](#host-configuration) instead.

Use the normal A/B lifecycle checks before modifying the target. If an
update has booted B but B is not committed, A is still the fallback and must
not be overwritten.

1. Collect the COSI and Host Configuration extensions. Validate their identities,
   paths and whether the host can activate them.
   Each boot path's loader must understand its slot's manifest version.
2. Select the inactive slot. Remove any old Slot Loader Manifest and synchronize
   its directory before clearing the Trident-owned extension directory,
   including interrupted-staging leftovers.
3. Reserve space to stage the new extension files. Write temporary `.raw` files
   inside the cleared directory, outside systemd's search paths. Verify hashes,
   filesystem structure and extension compatibility. Reject invalid images with
   an invalid-input error; flush valid files and directory entries to disk.
4. Build the manifest from the incoming extensions, not the old list. Once every
   image is ready, write and synchronize `slot-loader.json.tmp`, atomically
   rename it to `slot-loader.json`, then synchronize the slot directory.

Boot reads only `slot-loader.json`; a leftover temporary file is never used.

Finalization retains Trident's existing one-shot boot handoff. Before arming
the candidate, it verifies that persistent boot defaults still select the
committed OS and that the candidate supports the recovery path below.

#### Trident Slot Loader

##### Loader startup behavior

| Condition | Result |
| --- | --- |
| Store directory absent | Exit successfully (`0`); no activation. |
| Store present, manifest absent | Return nonzero; activation fails. |
| Valid manifest selects no images | Exit successfully (`0`); no activation. |
| Other lookup, parsing or selected-image errors | Return nonzero. |

Check store presence before slot detection. Successful early exits do not
create or replace runtime links or mounts. `{"version": 1}` is a valid empty
manifest; unreadable or malformed files are not.
Nonzero results follow the [update-boot failure policy](#update-boot-failure-recovery).

##### Loader configuration

ACL packaging supplies fixed loader settings:

```ini
LOADER_MODE=acl
TRIDENT_SYSEXT_STORE=/sysroot/var/lib/trident/slots
TRIDENT_SYSEXT_LINK_STORE=/var/lib/trident/slots
```

Both paths name the same store: `/sysroot/...` is the initrd's view;
`/var/lib/...` is the symlink destination after switching to the real root.
These settings are private. `LOADER_MODE` controls slot detection, not service
timing.

This RFC proposes a private kernel command-line input,
`trident.update_boot=1`, for an uncommitted A/B update boot. Absence means a
normal boot. It is boot configuration, not a manifest field or Host
Configuration option, so a missing or malformed manifest cannot hide it.

Trident stages a removable, prebuilt `trident-update.addon.efi` on the
candidate UKI's `.extra.d/` only, following the image's normal signing policy.
Factory and fallback entries do not carry it. Commit durably removes that copy
before any persistent boot-order/default promotion; subsequent boots
therefore use the normal failure policy.

##### Slot Loader Manifest

The [JSON Schema](./0838-slot-loader-manifest.schema.json) defines format v1
as UTF-8 JSON:

```json
{
  "version": 1,
  "sysexts": [{"name": "runtime"}],
  "confexts": [{"name": "runtime-defaults"}]
}
```

| Field | Required | Contract |
| --- | --- | --- |
| `version` | Yes | Integer `1`: file-format version, not an update or generation counter. |
| `sysexts`, `confexts` | No | Arrays of image objects; omitted arrays are treated as `[]`. Names must be unique per kind. |
| `name` | Yes, per item | Filename stem shared by the image `<name>.raw` and metadata `extension-release.<name>`. No directory separators, control characters, `.` or `..`. |

Unknown fields are ignored in both the manifest and image objects. Compatible
optional fields can be added within v1; changes that old loaders must understand
require a new format version.

An entry named `runtime` resolves to `<slot>/sysexts/runtime.raw` or
`<slot>/confexts/runtime.raw`, according to its array, relative to the store.
There are no caller-supplied image paths.

Sysexts retain native systemd ordering. Trident writes confexts highest-priority
first using
[systemd's version-aware extension-name ordering](https://github.com/systemd/systemd/blob/v255/src/sysext/sysext.c#L869-L892);
the loader uses that array order for `/etc` lowerdirs, where the first wins.

The loader rejects unsupported versions, missing required fields, invalid known
fields, duplicate known JSON keys and duplicate names within either kind, even if
ignored fields differ. It requires readable, regular image files confined to
the selected slot's directory and applies the
compatibility checks below before activation. Symlinks, missing image files and invalid
metadata fail activation; none trigger a fallback to directory scanning.

##### Selecting extensions at boot

1. **Existing:** ACL mounts ROOT in the initrd.
2. **New:** `trident-slot-loader.service` runs after that mount. The loader reads
   `acl.slot=a|b`, loads the slot's manifest, validates its images and prepares
   `/run/extensions` links and confext lowerdirs. Missing, invalid or duplicate
   slot values are errors.
3. **Integration change:** the existing `initrd-setup-root.service` gains a
   required dependency on the loader and consumes its lowerdir list when
   creating the existing writable `/etc` overlay.
4. **Existing:** `/run` survives switch-root, and `systemd-sysext.service` merges
   the sysexts in the real root. Afterwards, `ensure-sysext.service` reloads unit
   discovery and restarts targets; it does not perform the merge itself.

On rollback, the booted slot selects its old set before consumers run. No later
`trident commit` repair is needed. Sysext content is not available before the
native merge in step 4.

Loader failure must stop root setup; activation failure must prevent a healthy
commit. Use required service dependencies, not `Wants=` alone or conditions that
quietly skip the loader. Preserve another component's activation entries or
reject the conflict; the exact shared-directory publication protocol remains open.

##### Update-boot failure recovery

The loader unit uses `Restart=no` and gains
`OnFailure=trident-slot-loader-recovery.service`. This new initrd-only recovery
unit uses `ConditionKernelCommandLine=trident.update_boot=1` and
`ConditionPathExists=/etc/initrd-release`, with `DefaultDependencies=no`.
It has no dependency on `/etc` assembly or Trident's datastore and runs with
a bounded timeout. Nonzero exits, crashes and timeouts take this path, not
just errors returned by the loader's main function.

The handler captures diagnostics, then requests a reboot through systemd.
`SuccessAction=reboot` and `FailureAction=reboot` ensure that capture failure
or timeout does not leave the candidate stuck. It never re-arms the one-shot
entry or guesses the other slot: the consumed one-shot handoff returns the next boot
to the committed default. Normal boots keep the existing fatal-error behavior
without automatic reboot, preventing a fallback boot from entering a loop.

Before reboot, the handler overwrites a bounded, human-readable record at
`/sysroot/var/lib/trident/diagnostics/slot-loader-last-failure.log` and
synchronizes it. This is shared ROOT, outside the slot directory that staging
clears; the old OS reads it at `/var/lib/trident/diagnostics/slot-loader-last-failure.log`.
Record the boot ID, attempted slot when known, failure phase, service result
and error reason, not manifest or image contents. Failure to persist the record
is reported to the journal/console but does not prevent reboot. If the loader
crashed before reporting a reason, capture its unit result and journal tail.

Use leveled stderr diagnostics (slot, manifest path, store, activation method and
image errors), captured by the journal/console. Return nonzero on failure.
Install the loader's dracut module before normal UKI build and signing.

##### Confext handling

The loader mounts selected confext images read-only below
`/run/trident-slot-loader/etc/`. It publishes their `/etc` subdirectories as a
colon-separated lowerdir list in `/run/trident-slot-loader/etc-lowerdirs`, in
manifest order, only after all selected mounts are ready.

The loader reuses confext images, not systemd 255's native `systemd-confext`
merge, which would make `/etc` read-only. It does not create the final overlay;
ACL Bootengine consumes the list. With no confexts, there are no extra lowerdirs.

#### ACL Bootengine

ACL already combines factory defaults with a persistent writable directory
using overlayfs. This RFC requires two integration changes:

- The existing `initrd-setup-root.service` gains `Requires=` and `After=`
  dependencies on `trident-slot-loader.service`.
- The existing `initrd-setup-root` command reads the published lowerdir list
  before its overlay mount and prepends those paths to the factory lower.

The resulting mount keeps the existing upper and work directories:

```text
lowerdir=<published confext lowerdirs>:/sysroot/usr/share/distro/etc
upperdir=/sysroot/etc
workdir=/sysroot/.etc-work
```

A missing or empty list leaves the existing factory-only lower unchanged.
Bootengine does not modify a live mount or copy defaults into the writable
directory. Each confext supplies a directory to the same overlay, not another
overlay mount.

`overlaycleanup()` currently compares user files only with factory defaults.
How cleanup handles changed or removed extension defaults and deletion markers
(whiteouts) remains an open policy decision.

### Safety checks

| Area | Required behavior | Failure behavior |
| --- | --- | --- |
| Mode and layout | Validate selected inputs and placement before writing. `managed`/`acl` initially requires ACL A/B servicing and compatible boot components. | Reject unsupported layouts; never silently switch modes. Intentional skips are reported. |
| Native placement | Bundled `simple` and `native` images go only to writable A/B filesystems. Resolve the actual backing filesystem, including an overlay's writable upper. | Reject an unsafe explicit destination or an exhausted native candidate list before staging. |
| Ownership and cleanup | Clear only the inactive managed slot tree. Direct placement changes only Trident-tracked files, not entire native directories. Reject aliases, overlapping paths, traversal and untrusted writers. | Preserve the running/fallback slot and unrelated data; refuse unsafe cleanup. |
| Image validity | Validate delivery hashes and standard [`extension-release` compatibility](https://www.freedesktop.org/software/systemd/man/latest/os-release.html): distro, level/version, architecture and scope. `ID=_any` is allowed, not required. | Reject invalid selected images. Custom mounting does not bypass these checks. |
| Publication and activation | Keep temporary images undiscoverable and publish a complete managed manifest last. A valid empty manifest or absent store is a no-op; a missing manifest in an existing store is not. | Block root setup and healthy commit after activation failure; apply the update-boot recovery policy when marked. |
| Update recovery | Verify the committed default, one-shot support, recovery unit and marker addon before activation. Remove the marker before persistent boot promotion. | Refuse an unsafe handoff. Pending-boot failures record diagnostics and reboot once; normal-boot failures do not loop. |
| Writable-store trust | Delivery checks do not add boot-time hashes or authenticate the manifest. Signing, integrity and measurement remain platform policies; sealed `/usr` attestation does not cover the store. | Do not claim inherited trust. Loss of shared storage can affect both slots; A/B cannot restore it, and boot must not require a network download. |
| SELinux | Preserve Enforcing mode and correct payload/mount labels. | Surface access or labeling errors; never disable checks or exempt COSI inputs. |

## Public API Design

### Host Configuration

```yaml
image:
  url: https://example.com/acl.cosi
  sha384: <metadata-sha384>
os:
  managedExtensions: acl
```

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `os.managedExtensions` | Enum | `disabled` | Bundled-extension intake and placement policy. |

The proposed names distinguish filesystem-level slotting (`native`) from
Trident-managed logical slots on shared storage (`managed`). `acl` is an alias
for `managed`, not a separate mode. Aliases are normalized before comparison
and serialized as the canonical value, so changing only the spelling does not
trigger servicing. Boolean values are not mode values.

| Value | COSI-bundled extensions |
| --- | --- |
| `disabled` (default) | Ignore bundled extensions: do not extract, verify or deploy their payloads. OS-image processing and explicit Host Configuration extensions are unchanged. |
| `simple`\* | Place an image at its requested `path` only when that destination is writable and A/B-slotted. Entries without `path` are skipped with a diagnostic; an invalid or unsafe explicit path fails validation. |
| `native`\* | Consume all bundled images. For each kind, choose the first writable, A/B-slotted native directory from the ordered list below. Incoming destination hints do not choose the location. |
| `managed` (alias `acl`) | Consume all bundled images and own their store, lifecycle and activation, overriding requested destinations. Trident supplies logical slot separation on shared storage; no writable filesystem-level A/B store is required. The initial ACL store is fixed at `/var/lib/trident/slots`. |

\* Future developments, not part of the initial implementation.

The mode applies to both sysexts and confexts. In `managed`/`acl`, explicit
Host Configuration URL/OCI images join the owned set and use managed
destinations; in other modes their existing explicit placement behavior is
retained. Unrelated factory/operator extensions are not adopted.

Ignoring bundled payloads does not waive normal COSI metadata/OS-image
validation. Intentionally ignored members are not reported as unused OS images.
An empty `managed`/`acl` set publishes an empty target manifest.

### Native mode directory selection

| Kind | Candidate order |
| --- | --- |
| Sysext | `/var/lib/extensions`, then `/etc/extensions` |
| Confext | `/var/lib/confexts`, then `/usr/lib/confexts`, then `/usr/local/lib/confexts` |

The primary candidates retain Trident's existing default directories.
Ephemeral `/run` locations and initrd-scoped `/.extra/sysext` are not candidates.
An eligible directory exists or can be created on the target slot's writable
A/B filesystem without breaking image protection. Evaluate both kinds before
writing; if a consumed kind has no eligible destination, fail the update.
`native` never falls back to `managed` implicitly. On ACL, shared ROOT and
sealed `/usr` provide no eligible native destination.

`simple` and `native` only place files in the target filesystem if it is
A/B-slotted at a partition level. The image's existing boot setup activates
them: standard OS discovery for
`native`, or image-provided configuration for `simple`. Trident Slot Loader
does not activate these files.

### Identity and layout

For consumed images, use `(kind, id)` for update identity and verify it against
the DDI's `SYSEXT_ID` or `CONFEXT_ID`. Derive `<name>.raw` from
`extension-release.<name>`; ID and filename need not be equal.

```text
/var/lib/trident/slots/{a,b}/{sysexts,confexts}/<name>.raw
```

Record both ID and name. Reject duplicate IDs per kind and conflicting discovery
names/destinations, including existing activation entries. No source wins a
collision silently. In `managed`/`acl`, each slot's directory is exclusively
Trident-owned and is cleared when that slot is reused. The direct modes track
their deployed files without owning the surrounding native directory.

## Testing and Metrics

### Validation

A proof of concept combining the core managed-extension, A/B update and
writable `/etc` mechanisms was successfully executed on ACL.

### Required tests

For the initial ACL implementation, cover:

#### API and intake

- `managed`/`acl` equivalence and canonical serialization; `disabled` ignores
  bundled payloads without unused-image errors. Unsupported modes and Boolean
  inputs fail, while explicit URL/OCI behavior remains unchanged by default.
- Both input sources, destination overrides, IDs distinct from filenames,
  renames and cross-source collisions; unrelated extensions remain untouched.
- Compressed COSI and raw Host Configuration hashes, malformed images and
  extension compatibility failures are checked before publication.

#### Store lifetime and publication

- Fixed ROOT placement and mount ordering; refuse writes through aliases,
  traversal or unowned paths, or cleanup of a still-needed fallback slot.
- Repeated A/B reuse, additions/removals, empty sets, retries and mode changes:
  only the target changes, space stays bounded by two slot sets, and retained
  rollback data remains intact. Non-owned activation entries are preserved or
  rejected.
- Interrupt initial store creation, staging and manifest publication:
  retained slots remain bootable, temporary manifests are never consumed,
  and no partial set becomes active.

#### Loader and ACL integration

- Manifest versions/types and required fields, duplicate known keys/names
  (including different ignored fields), unknown optional fields, unsafe names
  and symlinks; omitted arrays and version-only manifests are valid.
- No store or a valid empty set exits successfully without links or mounts.
  A present store with missing/malformed/unreadable metadata or selected images
  fails rather than activating a partial set.
- ROOT/loader/Bootengine ordering, factory-only fallback for missing/empty
  lowerdir lists, and blocked root setup after loader failure.
- Multiple `/etc` layers, precedence, copy-up, whiteouts and removal; user edits
  survive update and rollback. Assert visible content, not just file existence.

#### Update failure and rollback

- Loader errors, crashes and timeouts return a pending boot to the committed
  OS without a loop; normal no-ops do not reboot. Include bootloader fallback,
  failed commit and power loss in both slot directions.
- Marker cleanup precedes persistent boot promotion; cleanup failure preserves
  the previous default.
- Failure records survive outside the reused slot. Read-only ROOT, capture
  errors and timeouts do not prevent the rollback reboot.

#### Image construction and compatibility

- Base/customized images boot and reboot before datastore initialization,
  with absent, empty or populated stores. Construction edits keep payloads and
  manifests consistent without adopting unrelated images.
- Old/new loader, manifest, boot-context and Host Configuration reader
  combinations preserve supported rollback paths; unsafe adoption is refused.

#### Performance

- Matched cold/warm benchmarks for the scenarios and budgets below, including
  image-count scaling and end-to-end critical-path measurements.

### Metrics

| Scenario | Provisional p95 goal | What is measured |
| --- | --- | --- |
| No-op: absent store or valid empty manifest | Less than 5 ms added latency | Service/process startup and lookup or empty-manifest handling; no links or mounts. |
| Sysext-only | Less than 10 ms added latency | All loader work before the existing native sysext merge. The unchanged OS merge is not charged as new loader work. |
| Confext-only or mixed | Less than 10 ms coordination time, plus measured image preparation | Separate startup, selection and publication from image mounting/validation. Report the full time until `/etc` is ready; its total budget is TBD after representative measurements. |

Confext handling owns work that systemd would normally perform, so its total
time is inherently higher than selection alone and is not subject to the
sysext-only 10 ms limit. Do not report coordination time as the whole impact.

Compare matched ACL boots with equivalent hardware, kernel, services and
payloads. Measure added boot-critical-path delay, not just process CPU time;
report cold/warm median and p95, preparation time and image counts. The no-op
5 ms goal is a starting budget, and none of these targets is yet demonstrated.

Record staging, activation and rollback outcomes with slot, selected image names,
activation method and failure reason in servicing logs and boot diagnostics.
Correlate a recovered update failure with its persistent record's boot ID.
Success requires matching visible content and no healthy commit after activation
failure.

## Servicing

### Introduction and opt-in

The default is no new behavior: omitted means `disabled`, existing OS
servicing and explicit URL/OCI declarations are unchanged, and bundled images
are not adopted. Images without a store boot normally without Trident
initialization.

Customers opt in with `os.managedExtensions: managed` or its `acl` alias:

1. Supply an ACL image with loader, Bootengine and recovery support. Preflight
   checks the target and retained rollback paths.
2. On first use, publish the store only after retained slots have valid
   manifests, empty where appropriate.
3. Stage the target and attempt a one-shot boot. The committed OS stays the
   fallback until health checks and commit succeed.

### Compatibility policy

Keep the loader and its boot contracts small, stable and long-lived.

| Interface | Compatibility approach |
| --- | --- |
| Host Configuration | The enum allows future modes without changing the field shape. Initially support `disabled` and `managed`/`acl`; reject unsupported values. Canonicalize aliases and omit defaults from serialized state. |
| Loader and manifest | New loaders retain older released format readers. Old loaders ignore optional unknown fields within supported versions; incompatible semantics need a new version that they reject. |
| Per-slot state | Write each slot's manifest in a format its loader supports. Preserve the retained slot's files and format; never convert it to a new-only version. |
| Boot and store contracts | Keep paths, update markers and failure semantics stable. Rollback selects matching extensions before consumers run, without late `trident commit` repair. |

Rollback needs compatible Trident state readers as well as a compatible
loader: older strict `Os` readers may reject new fields or values.

### Preserving rollback capability

**TBD:** the exact compatibility matrix and admission/upgrade checks that prove
rollback remains usable. Prefer an exceptionally stable loader, maintained
backward compatibility and automated old/new boot tests. Block changes whose
retained rollback path is incompatible.

### Policy changes

Mode and managed-set changes initially use A/B servicing; alias spelling alone
does not trigger one. Preserve retained slots until the transition completes.
Normal inactive-slot reuse replaces its old set rather than retaining history.

If an owned store already exists and the target mode is not `managed`/`acl`,
clear the reused target's owned tree and publish an empty manifest. This avoids
stale activation without consuming disabled bundles or wiping native
directories. Migrate legacy aliases safely: deleting them can break rollback,
while leaving them can shadow `/run`. Refuse unsafe transitions and leave
unrelated entries untouched.

Reject unsupported Runtime Update requests, without promotion or partial application.
Explicit Host Configuration inputs keep 0789's servicing constraints;
`simple`/`native` remain deferred.

## Implementation Plan

Initial delivery is `managed`/`acl` on ACL, with `disabled` as the default
for bundled extensions. `simple`, `native`, configurable stores and native
mutable-confext migration remain future work. ACL integration of
`trident offline-initialize` is an independent goal, not a prerequisite for
unmanaged-image boot.

RFC 0789 reader/writer support is a prerequisite; its shared work and the
separate IC authoring API are not estimated here. Sizes below are relative
design/implementation effort (S/M/L), not elapsed-time commitments.

| Phase | Work and completion gate | Size | Main risk |
| --- | --- | :-: | --- |
| 1. Contracts and compatibility | Freeze the initial API, manifest and boot interfaces; define the rollback compatibility matrix and admission rules. | M/M | High: retained readers |
| 2. Managed staging and lifecycle | Validate inputs; implement fixed-ROOT placement, atomic first-use publication, inactive-slot replacement and safe disable transitions. | M/L | High: ownership and interrupted writes |
| 3. Loader and ACL integration | Implement selection, links and confext preparation; integrate Bootengine ordering/lowerdirs and package the dracut assets before UKI signing. | M/M | High: early-boot integration |
| 4. Update-boot recovery | Add the candidate marker, one-shot fallback, persistent diagnostics and marker removal before commit promotion; prove fallback cannot loop. | M/M | High: commit and failure ordering |
| 5. Rollout and fault validation | Apply target/retained-reader preflight and execute the grouped functional, interruption, compatibility and rollback tests. | L/L | High: rollback viability |
| 6. Performance validation | Measure no-op and sysext-only budgets plus complete confext cost; publish cold/warm results and settle the confext total budget. | S/M | Medium: hardware and image-count variance |

Production enablement requires passing the initial test areas, demonstrated
rollback compatibility and published measurements against the scenario budgets.
Unmet performance goals need explicit review; the compatibility-enforcement
TBD must be resolved before that gate. No new COSI fields, systemd source
changes or cluster-control-plane integration are required.

## Counter-Arguments

### Drawbacks

- The shared store is a failure domain for both slots and needs space for both
  slot sets.
- The loader and versioned manifests add early-boot and rollout complexity.
- Confext layering requires `/etc` reconciliation and integrity policies.

### Alternatives

- **Compatibility-based selection:** versions may match, and portable images may
  be compatible with both slots.
- **Version selection (`.v` or "newest" links):** selects a version, not the
  booted OS slot.
- **New dedicated A/B extension partitions:** valid, but require reserving disk
  capacity for both slots upfront. Shared ROOT lets the two sets use available
  filesystem space as needed.
- **ESP payload storage:** unnecessary when the existing ROOT can hold the images.
- **Systemd backports or changing ACL's merge order:** neither is required by
  this design.

## Open Questions

1. Shared activation-path publication and legacy alias migration.
2. `/etc` reconciliation and integrity/attestation policy.
3. Whether managed sysext-only Runtime Update belongs in the first release;
   this proposal starts with A/B servicing.
4. How rollback compatibility is admitted and enforced across loader, manifest,
   boot-context and servicing-state reader versions.

## Future Possibilities

### Configurable store location

A configurable `os.managedContentStore` path could support other storage
layouts later. It would require early-mount dependencies, compatible loader
configuration and rollback-safe migration of both slot sets. The initial ACL
implementation supports only `/var/lib/trident/slots` on ROOT.

### Direct placement modes

Future `simple` and `native` implementations need placement tests for pathless
entries, unsafe explicit paths, candidate ordering/exhaustion, writable
A/B-partition detection and direct activation without the loader.

### Native mutable-confext integration

Newer ACL can replace custom mounting with native mutable confext integration
without changing the API or payload format. Verify precedence, upper state,
labels, compatibility and ordering; version alone is insufficient.

### Runtime updates

Managed sysext-only Runtime Update can follow; ACL cannot replace its `/etc`
lowerdirs live.

### Btrfs-backed managed store

On Btrfs-backed storage, the store could use a pair of subvolumes, one per
slot, to simplify inactive-slot replacement without changing paths or the
public API.

### Other platforms

Non-ACL backends can reuse the policy once validated against their storage
and activation models.

A future `managed` backend could prepare links in writable A/B extension
search directories during staging. Switching OS slots then selects those
links without a loader. The owned images may live on shared or slotted
storage, but the links must still resolve after reboot.

Choose separately for sysexts and confexts. Native confext integration can use
slotted links or boot-selected `/run/confexts` entries if it preserves the
platform's `/etc` mutability and user-state persistence. Prefer existing
writable A/B directories in a fixed, platform-defined order.
