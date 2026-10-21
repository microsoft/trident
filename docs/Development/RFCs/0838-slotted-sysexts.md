# 0838 Trident-Managed Extensions

- Date: 2026-10-15
- RFC PR:
  [microsoft/trident#838](https://github.com/microsoft/trident/pull/838)
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
RFC 0789 proposes COSI 1.3 support for bundled extensions: optional metadata
arrays `extensions.sysexts` and `extensions.confexts` reference compressed DDIs
by ID, location, size and hash. OS and extensions can then ship as one artifact;
Trident can also fetch extensions listed separately in Host Configuration.

#### ACL storage and update behavior

Azure Container Linux (ACL) already uses sysexts for containerd and OEM
integration and is expected to rely more on them.

Only the `/usr` filesystem is A/B-paired and dm-verity-protected, with paired
hash partitions; **all mutable filesystems are shared**. ESP, OEM and ROOT are
unpaired, and `/etc`, `/var` and `/opt` live on shared ROOT.

Images embedded in `/usr` follow its slot. For images on shared storage,
changing the stored files affects both slots; switching `/usr` does not undo
those changes.

#### Flatcar's approach

Flatcar's updater fetches OEM and optional release sysexts as
[separate signed payloads](https://github.com/flatcar/update_engine/blob/f23d6ea848ffe2c8721bc49d2a9f77f4de038ad2/flatcar-postinst#L77-L240),
advertised through Nebraska's Omaha response rather than bundled with the OS
partition update. Its
[initrd preparation](https://github.com/flatcar/bootengine/blob/7727ec78da72e700e8fa1ce2144cb2476448d186/dracut/99setup-root/initrd-setup-root-after-ignition)
selects matching versioned images from shared storage using the booted `/usr`
version and recreates `/etc/extensions` symlinks. Users must separately
provision third-party sysexts and
[configure their updates](https://www.flatcar.org/docs/latest/sys-ext/#updating-custom-sysext-images),
for example with `systemd-sysupdate`; they do not automatically join OS
servicing.

Flatcar uses native `systemd-confext` for factory `/etc` defaults, with
[mutable mode](https://github.com/flatcar/init/blob/0765e955aca24034d66b9389e0d538e4c3ee543c/systemd/confext.conf)
and a persistent writable upper. This relies on support
[introduced upstream in systemd v256](https://github.com/systemd/systemd/blob/v256/man/systemd-sysext.xml).
ACL's v255 baseline lacks it. This RFC avoids a systemd upgrade or backport
because of the added complexity and stability risk, retaining ACL's existing
writable `/etc` overlay instead. Flatcar's
[boot code](https://github.com/flatcar/bootengine/blob/7727ec78da72e700e8fa1ce2144cb2476448d186/dracut/99setup-root/initrd-setup-root)
prepares `/etc` and merges confexts/sysexts in the initrd.

Both designs keep extension files on shared storage and choose which to load at
boot. Flatcar chooses by OS version; this RFC chooses by slot, allowing
different extension sets for the same OS version and linking them to the A/B
status of the OS. This boot-time selection is a potential area for shared
Bootengine code. ACL would still use its existing writable `/etc` overlay and
merge sysexts after switch-root.

### Problem Statement

Users build custom images on ACL, often adding loose files or preloaded sysexts
on mutable storage. ACL's A/B update replaces the core OS, not those additions.
Their stack therefore needs an out-of-band updater even though it already ships
in their image.

### Goals

- Update and roll back ACL's OS and declared sysexts together, using one COSI
  instead of an out-of-band stack updater.
- Onboard ACL to managed sysexts by default. Avoid regressions to non-ACL AzL
  targets.
- Support ACL's partition layout, where all writeable partitions are NOT
  A/B-slotted.
- Activate the matching extension set before consumers run on every boot,
  including rollback.
- Preserve user state while confext defaults follow the OS slot.

## Scope

**ACL is the initial design and implementation target** because its sealed A/B
`/usr` and shared mutable storage require managed slot selection. Support for
other platforms is future work. The public API avoids ACL-specific settings so
it can be reused later.

### Requirements

- Support ACL's shared storage without new partitions or systemd source changes.
- Let users explicitly enable Trident to update bundled extensions alongside the
  OS.
- Base ACL images must continue to boot as-is.
- Clean and reuse the inactive slot for staging an update's extensions, ensuring
  the active slot's extensions remain usable and available for any eventual
  rollback.
- Preserve each platform's filesystem mutability and user-state persistence.
- (Future) Make this sysext management feature reusable on platforms with
  writable A/B storage, not only ACL.

### Out of Scope

- Image Customizer's managed-content authoring/bundling API and build design (a
  required separate proposal; see [Dependencies](#dependencies)).
- Relocating the Trident-owned store; see
  [Future Possibilities](#future-possibilities).
- Updating arbitrary files on shared mutable storage.
- Runtime Update for managed extension changes in the initial release. Allowing
  for runtime updates of managed extensions should be planned in a standalone
  document.
- New image-less updates, initrd-scoped sysext content or ESP payload storage.
- Bundled disk streaming; 0789's exclusion remains.

### Exit Criteria

- A COSI file carrying extensions completes an ACL A/B update with the OS,
  sysexts and confexts staged for the same target slot.
- Update, commit, health-check rollback and bootloader fallback expose matching
  slot content before consumers run, without partial activation or late repair.
- Repeated slot reuse removes obsolete extension files without changing the
  active slot or accumulating older sets.
- The public API, safe adoption and tests below are delivered; user `/etc` edits
  and SELinux Enforcing are preserved.

## Dependencies

- **[RFC 0789](https://github.com/microsoft/trident/pull/789):** COSI extension
  metadata contract and reader/writer support.
- **Image Customizer (IC):** a separate, out-of-scope API and build design for
  declaring extensions, maintaining the store and manifests, and bundling
  extensions into COSIs. Required for the end-to-end image-building workflow;
  Trident accepts a conforming COSI from any producer.

## Implementation

### Design

Trident keeps the old and new extension files side by side. An update prepares
the inactive slot's files without changing the running system. At boot,
`trident-slot-loader` selects the files for the OS slot that actually booted.
Rollback uses the same selection process.

![Managed extension delivery and activation on ACL](./0838-slotted-sysexts/0838-managed-extensions-overview.svg)

#### Where the files live

The [store](#managed-content-store) is fixed at `/var/lib/trident/slots`. Its
`a/` and `b/` directories hold each slot's sysexts and confexts on shared ROOT.
These are separate directories, not new partitions. If A is running, an update
prepares B's set while retaining A's files. **Reusing B replaces its entire
managed extension set**, just as staging a new OS replaces B's partition
contents. Keep one set per slot, not a history of sets within each slot.

The [staged update](#trident-staging) takes effect on reboot into the target OS
slot.

#### Booting after an update

The **[Slot Loader Manifest](#slot-loader-manifest)** tells the loader exactly
which managed extensions belong to the booted slot. It is created when the store
is initialized, during
[image construction](#base-image-construction-and-ic-integration) or first
servicing, and replaced during update staging. No additional COSI format change
is required.

After reading `acl.slot`, the loader opens
`/sysroot/var/lib/trident/slots/<slot>/slot-loader.json` in the initrd. It uses
this file, not Trident's SQLite database or a directory scan.

The loader [returns success without activation](#loader-startup-behavior) when
the store is absent or a valid manifest selects no images. A loader failure on a
newly deployed slot follows the
[update-boot failure policy](#update-boot-failure-recovery).

#### Writable `/etc`

User edits in the writable **upper** take precedence over
[extension defaults](#confext-handling), which take precedence over factory
defaults. Changing OS slots changes the extension defaults but keeps that shared
upper, so user edits survive. The native sysext merge still runs after `/etc`
assembly.

![Final /usr and /etc layers, backed by the booted slot's verity-protected /usr partition](./0838-slotted-sysexts/0838-filesystem-layer-stack.svg)

### Component deep dives

#### Managed content store

The managed content store is persistent storage for Trident-managed content
associated with an OS slot. In this design, it holds each slot's sysext/confext
`.raw` images and Slot Loader Manifest (`slot-loader.json`). The initial ACL
implementation uses only `/var/lib/trident/slots`, with separate `a/` and `b/`
directories on shared ROOT.

The path must resolve to ACL's shared ROOT filesystem. ROOT must be mounted
before the loader runs and writable during staging. Target-slot selection comes
from the host's A/B configuration, not source COSI partition numbers.

#### Base Image Construction and IC Integration

ACL images may omit the managed content store entirely. **If a store is present,
it must contain valid manifests for every bootable slot**, even if they select
no images. `{"version": 1}` is a valid empty manifest. Boot must not depend on
Trident servicing or datastore initialization.

IC must support adding, replacing and removing managed payloads while publishing
their files and manifests together. Its API remains a
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

Use the normal A/B lifecycle checks before modifying the target. If an update
has booted B but B is not committed, A is still the fallback and must not be
overwritten.

1. Collect the COSI and Host Configuration extensions. Validate their
   identities, paths and whether the host can activate them. Check that the
   loader in the target boot image supports the manifest format being written.
2. Select the inactive slot. Remove any old Slot Loader Manifest and
   `slot-loader.pending` marker, then synchronize its directory before clearing
   the Trident-owned extension directory, including interrupted-staging
   leftovers.
3. Reserve space to stage the new extension files. Write temporary `.raw` files
   inside the cleared directory, outside systemd's search paths. Verify hashes,
   filesystem structure and extension compatibility. Reject invalid images with
   an invalid-input error; flush valid files and directory entries to disk.
4. Build the manifest from the incoming extensions, not the old list. Once every
   image is ready, write and synchronize `slot-loader.json.tmp`, atomically
   rename it to `slot-loader.json`, then synchronize the slot directory. Write
   and synchronize a `slot-loader.pending` marker beside the published manifest.

Boot reads the manifest and marker; a leftover temporary file is never used.

Finalization retains Trident's existing one-shot boot handoff. Before arming the
candidate, it verifies that persistent boot defaults still select the committed
OS, the marker is durable and the candidate supports the recovery path below.

#### Trident Slot Loader

##### Loader startup behavior

| Condition | Result |
| --- | --- |
| Store directory absent | Exit successfully (`0`); no activation. |
| Store present, manifest absent | Return nonzero; activation fails. |
| Valid manifest selects no images | Exit successfully (`0`); no activation. |
| Other lookup, parsing or selected-image errors | Return nonzero. |

Check store presence before slot detection. Successful early exits do not create
or replace runtime links or mounts. `{"version": 1}` is a valid empty manifest;
unreadable or malformed files are not. Nonzero results follow the
[update-boot failure policy](#update-boot-failure-recovery).

##### Loader configuration

ACL packaging supplies an initrd-readable loader env file:

```ini
LOADER_MODE=acl
TRIDENT_SYSEXT_STORE=/sysroot/var/lib/trident/slots
TRIDENT_SYSEXT_LINK_STORE=/var/lib/trident/slots
ROLLBACK_ON_LOADER_FAILURE=true
```

Both paths name the same store: `/sysroot/...` is the initrd's view;
`/var/lib/...` is the symlink destination after switching to the real root. The
paths and `LOADER_MODE` are fixed; `LOADER_MODE` controls slot detection, not
service timing. `ROLLBACK_ON_LOADER_FAILURE` defaults to `true` and can be set
to `false` in this env file to leave a failed boot stopped for inspection. The
failure handler reads this setting independently of the loader. The
`slot-loader.pending` marker beside each slot's manifest identifies a new slot
boot without opening Trident's SQLite datastore in the initrd. No new kernel
command-line option or UKI addon is required; `acl.slot` selects the slot.

##### Slot Loader Manifest

The [JSON Schema](./0838-slotted-sysexts/0838-slot-loader-manifest.schema.json)
defines format v1 as UTF-8 JSON:

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

The manifest is written once for each staged set. Boot, commit and rollback do
not rewrite it; it changes only when that slot is reused.

An entry named `runtime` resolves to `<slot>/sysexts/runtime.raw` or
`<slot>/confexts/runtime.raw`, according to its array, relative to the store.
There are no caller-supplied image paths.

Sysexts retain native systemd ordering. Trident writes confexts highest-priority
first using
[systemd's version-aware extension-name ordering](https://github.com/systemd/systemd/blob/v255/src/sysext/sysext.c#L869-L892);
the loader uses that array order for `/etc` lowerdirs, where the first wins.

The loader rejects unsupported versions, missing required fields, invalid known
fields, duplicate known JSON keys and duplicate names within either kind, even
if ignored fields differ. It requires readable, regular image files confined to
the selected slot's directory and applies the compatibility checks below before
activation. Symlinks, missing image files and invalid metadata fail activation;
none trigger a fallback to directory scanning.

##### Selecting extensions at boot

1. **Existing:** ACL mounts ROOT in the initrd.
2. **New:** `trident-slot-loader.service` runs after that mount. The loader
   reads `acl.slot=a|b`, loads the slot's manifest, validates its images and
   prepares `/run/extensions` links and confext lowerdirs. Missing, invalid or
   duplicate slot values are errors.
3. **Integration change:** the existing `initrd-setup-root.service` gains a
   required dependency on the loader and consumes its lowerdir list when
   creating the existing writable `/etc` overlay.
4. **Existing:** `/run` survives switch-root, and `systemd-sysext.service`
   merges the sysexts in the real root. Afterwards, `ensure-sysext.service`
   reloads unit discovery and restarts targets; it does not perform the merge
   itself.

On rollback, the booted slot selects its old set before consumers run. No later
`trident commit` repair is needed. Sysext content is not available before the
native merge in step 4.

Loader failure must stop root setup; activation failure must prevent a healthy
commit. Use required service dependencies, not `Wants=` alone or conditions that
quietly skip the loader. Leave unrelated `/run/extensions` entries untouched.
Fail if a required symlink would overwrite one.

##### Update-boot failure recovery

Staging writes `<slot>/slot-loader.pending` beside `slot-loader.json` before
arming the one-shot boot attempt. The loader removes the marker only after its
own successful execution, including a valid empty manifest, and synchronizes the
slot directory. This does not wait for the rest of the boot or for commit.
Loader failure leaves the marker in place.

On loader failure, crash or timeout, an initrd failure handler reads `acl.slot`
and checks that slot's marker without relying on manifest parsing or the loader
process. It logs the slot, boot ID, unit result and reason to the
journal/console. There is no separate failure record; initrd journal messages
may not survive a reboot unless journal persistence is configured.

If the marker exists and `ROLLBACK_ON_LOADER_FAILURE=true` (the default), the
handler requests a reboot. The one-shot attempt has already been consumed, so
the next boot selects the committed default. When the setting is `false`, the
failed boot stays stopped for inspection. A missing marker or invalid slot never
triggers an automatic reboot, preventing fallback failures from looping. The
handler does not re-arm the failed entry or restart the loader and must run
independently of `/etc` assembly.

Cancellation or restaging clears stale markers. A failed candidate's marker may
remain on shared ROOT after fallback, but only the booted slot's marker controls
recovery.

Use leveled stderr diagnostics (slot, manifest path, store, activation method
and image errors), captured by the journal/console. Return nonzero on failure.
Install the loader's dracut module before normal UKI build and signing.

##### Confext handling

The loader mounts selected confext images read-only below
`/run/trident-slot-loader/etc/`. It publishes their `/etc` subdirectories as a
colon-separated lowerdir list in `/run/trident-slot-loader/etc-lowerdirs`, in
manifest order, only after all selected mounts are ready.

The loader reuses confext images, not systemd 255's native `systemd-confext`
merge, which would make `/etc` read-only. It does not create the final overlay;
ACL Bootengine consumes the list. With no confexts, there are no extra
lowerdirs.

#### ACL Bootengine

ACL already combines factory defaults with a persistent writable directory using
overlayfs. This RFC requires two integration changes:

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

`overlaycleanup()` currently compares user files only with factory defaults. How
cleanup handles changed or removed extension defaults and deletion markers
(whiteouts) remains an open policy decision.

### Safety checks

| Area | Required behavior | Failure behavior |
| --- | --- | --- |
| Mode and layout | Validate selected inputs and placement before writing. `managed`/`acl` initially requires ACL A/B servicing and compatible boot components. | Reject unsupported layouts; never silently switch modes. Intentional skips are reported. |
| Native placement | Bundled `simple` and `native` images go only to writable A/B filesystems. Resolve the actual backing filesystem, including an overlay's writable upper. | Reject an unsafe explicit destination or an exhausted native candidate list before staging. |
| Ownership and cleanup | Clear only the inactive managed slot tree. Direct placement changes only Trident-tracked files, not entire native directories. Reject aliases, overlapping paths, traversal and untrusted writers. | Preserve the running/fallback slot and unrelated data; refuse unsafe cleanup. |
| Image validity | Validate delivery hashes and standard [`extension-release` compatibility](https://www.freedesktop.org/software/systemd/man/latest/os-release.html): distro, level/version, architecture and scope. `ID=_any` is allowed, not required. | Reject invalid selected images. Custom mounting does not bypass these checks. |
| Publication and activation | Keep temporary images undiscoverable and publish a complete managed manifest last. A valid empty manifest or absent store is a no-op; a missing manifest in an existing store is not. | Block root setup and healthy commit after activation failure; apply recovery only for an identified uncommitted candidate boot. |
| Update recovery | Verify the committed default, one-shot support, failure handler and durable `slot-loader.pending` marker before activation. Commit must complete the pending-state transition before persistent boot promotion. | Refuse an unsafe handoff. Candidate failures log to the journal and reboot when enabled; a fallback boot must not trigger another reboot. |
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
for `managed`, not a separate mode. Aliases are normalized before comparison and
serialized as the canonical value, so changing only the spelling does not
trigger servicing. Boolean values are not mode values.

| Value | COSI-bundled extensions |
| --- | --- |
| `disabled` (default) | Ignore bundled extensions: do not extract, verify or deploy their payloads. OS-image processing and explicit Host Configuration extensions are unchanged. |
| `simple`\* | Place an image at its requested `path` only when that destination is writable and A/B-slotted. Entries without `path` are skipped with a diagnostic; an invalid or unsafe explicit path fails validation. |
| `native`\* | Consume all bundled images. For each kind, choose the first writable, A/B-slotted native directory from the ordered list below. Incoming destination hints do not choose the location. |
| `managed` (alias `acl`) | Consume all bundled images and own their store, lifecycle and activation, overriding requested destinations. Trident supplies logical slot separation on shared storage; no writable filesystem-level A/B store is required. The initial ACL store is fixed at `/var/lib/trident/slots`. |

\* Future developments, not part of the initial implementation.

The mode applies to both sysexts and confexts. In `managed`/`acl`, explicit Host
Configuration URL/OCI images join the owned set and use managed destinations; in
other modes their existing explicit placement behavior is retained. Unrelated
factory/operator extensions are not adopted.

Ignoring bundled payloads does not waive normal COSI metadata/OS-image
validation. Intentionally ignored members are not reported as unused OS images.
An empty `managed`/`acl` set publishes an empty target manifest.

### Native mode directory selection

| Kind | Candidate order |
| --- | --- |
| Sysext | `/var/lib/extensions`, then `/etc/extensions` |
| Confext | `/var/lib/confexts`, then `/usr/lib/confexts`, then `/usr/local/lib/confexts` |

The primary candidates retain Trident's existing default directories. Ephemeral
`/run` locations and initrd-scoped `/.extra/sysext` are not candidates. An
eligible directory exists or can be created on the target slot's writable A/B
filesystem without breaking image protection. Evaluate both kinds before
writing; if a consumed kind has no eligible destination, fail the update.
`native` never falls back to `managed` implicitly. On ACL, shared ROOT and
sealed `/usr` provide no eligible native destination.

`simple` and `native` only place files in the target filesystem if it is
A/B-slotted at a partition level. The image's existing boot setup activates
them: standard OS discovery for `native`, or image-provided configuration for
`simple`. Trident Slot Loader does not activate these files.

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

A proof of concept combining the core managed-extension, A/B update and writable
`/etc` mechanisms was successfully executed on ACL.

### Required tests

Tests will be added for these areas:

- Configuration and image sources.
- Store replacement and interrupted updates.
- Manifest parsing and boot integration.
- Update failure and rollback.
- Image construction and version compatibility.
- Boot-time impact.

Details are in the supporting [test plan](./0838-slotted-sysexts/test-plan.md).

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
report cold/warm median and p95, preparation time and image counts. The no-op 5
ms goal is a starting budget, and none of these targets is yet demonstrated.

Record staging, activation and rollback outcomes with slot, selected image
names, activation method and failure reason in servicing logs and boot
diagnostics. Correlate a recovered update failure by boot ID when journal
messages survive. Success requires matching visible content and no healthy
commit after activation failure.

## Servicing

### Introduction and opt-in

The default is no new behavior: omitted means `disabled`, existing OS servicing
and explicit URL/OCI declarations are unchanged, and bundled images are not
adopted. Images without a store boot normally without Trident initialization.

Customers opt in with `os.managedExtensions: managed` or its `acl` alias:

1. Supply an ACL image with loader, Bootengine and recovery support. Preflight
   checks the target and retained rollback paths.
2. On first use, publish the store only after retained slots have valid
   manifests, empty where appropriate.
3. Stage the target and attempt a one-shot boot. The committed OS stays the
   fallback until health checks and commit succeed.

### Compatibility policy

*WIP: TODO: re-review*

Keep the loader and its boot contracts small, stable and long-lived.

The target image is normally expected to contain a loader from the same or a
newer release than the staging Trident. The actual check is format support:
Trident must write a manifest that the target loader can read, regardless of the
loader running on the servicing OS.

| Interface | Compatibility approach |
| --- | --- |
| Host Configuration | The enum allows future modes without changing the field shape. Initially support `disabled` and `managed`/`acl`; reject unsupported values. Canonicalize aliases and omit defaults from serialized state. |
| Loader and manifest | Check the target pair during staging. Optional unknown fields are ignored within supported formats; unsupported versions are rejected. A release number alone does not prove support. |
| Per-slot state | Publish the manifest once and retain it with its loader through boot, commit and rollback. Do not regenerate or upgrade the retained slot's manifest. |
| Boot and store contracts | Keep paths, marker lifecycle, one-shot handoff and failure semantics stable. Rollback selects matching extensions before consumers run, without late `trident commit` repair. |

Trident's shared servicing state is a separate compatibility concern: older
strict `Os` readers may reject new fields or values after rollback.

### Preserving rollback capability

*WIP: TODO: re-review*

Rollback returns to the retained loader and its unchanged manifest. If that pair
booted successfully before the update, manifest-version changes in the new slot
do not affect it. There is no need for the old loader to read the new slot's
manifest, or for the new loader to read the old slot's.

**TBD:** how staging discovers target-loader format support, and how preflight
checks shared boot configuration, store layout and servicing-state changes. Keep
those shared contracts stable; preserving a slot's pair does not protect it from
incompatible changes to shared state.

### Policy changes

*WIP: TODO: re-review*

Mode and managed-set changes initially use A/B servicing; alias spelling alone
does not trigger one. Preserve retained slots until the transition completes.
Normal inactive-slot reuse replaces its old set rather than retaining history.

If an owned store already exists and the target mode is not `managed`/`acl`,
clear the reused target's owned tree and publish an empty manifest. This avoids
stale activation without consuming disabled bundles or wiping native
directories. Migrate legacy aliases safely: deleting them can break rollback,
while leaving them can shadow `/run`. Refuse unsafe transitions and leave
unrelated entries untouched.

Reject unsupported Runtime Update requests, without promotion or partial
application. Explicit Host Configuration inputs keep 0789's servicing
constraints; `simple`/`native` remain deferred.

## Implementation Plan

Initial delivery is `managed`/`acl` on ACL, with `disabled` as the default for
bundled extensions. `simple`, `native`, configurable stores and native
mutable-confext migration remain future work. ACL integration of
`trident offline-initialize` is an independent goal, not a prerequisite for
unmanaged-image boot.

RFC 0789 reader/writer support is a prerequisite; its shared work and the
separate IC authoring API are not estimated here. Sizes below are relative
design/implementation effort (S/M/L), not elapsed-time commitments.

| Phase | Work and completion gate | Size | Main risk |
| --- | --- | :-: | --- |
| 1. Contracts and compatibility | Freeze the initial API, manifest and boot interfaces; define target-loader format detection and shared-state rollback checks. | M/M | High: staging and shared contracts |
| 2. Managed staging and lifecycle | Validate inputs; implement fixed-ROOT placement, atomic first-use publication, inactive-slot replacement and safe disable transitions. | M/L | High: ownership and interrupted writes |
| 3. Loader and ACL integration | Implement selection, links and confext preparation; integrate Bootengine ordering/lowerdirs and package the dracut assets before UKI signing. | M/M | High: early-boot integration |
| 4. Update-boot recovery | Integrate the `slot-loader.pending` marker, one-shot fallback, journal diagnostics and configurable reboot; prove fallback cannot loop. | M/M | High: state and failure ordering |
| 5. Rollout and fault validation | Verify the staged loader/manifest pair, preserve the retained pair and check shared-state readers; execute the grouped fault and rollback tests. | L/L | High: rollback viability |
| 6. Performance validation | Measure no-op and sysext-only budgets plus complete confext cost; publish cold/warm results and settle the confext total budget. | S/M | Medium: hardware and image-count variance |

Production enablement requires passing the initial test areas, demonstrated
rollback compatibility and published measurements against the scenario budgets.
Unmet performance goals need explicit review; target-loader detection and
shared-state compatibility checks must be defined before that gate. No new COSI
fields, systemd source changes or cluster-control-plane integration are
required.

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
- **ESP payload storage:** unnecessary when the existing ROOT can hold the
  images.
- **Systemd backports or changing ACL's merge order:** neither is required by
  this design.

## Open Questions

*WIP: TODO: re-review*

1. Shared activation-path publication and legacy alias migration.
2. `/etc` reconciliation and integrity/attestation policy.
3. Whether managed sysext-only Runtime Update belongs in the first release; this
   proposal starts with A/B servicing.
4. How staging discovers which manifest formats the target loader supports.
5. How changes to shared boot configuration, store layout and servicing state
   are checked for compatibility with the retained OS.
6. How pending-update context reaches the initrd and stays consistent through
   commit, cancellation and interrupted updates.

## Future Possibilities

### Configurable store location

A configurable `os.managedContentStore` path could support other storage layouts
later. It would require early-mount dependencies, compatible loader
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

An implementation of this could involve an extension-only COSI, or
OCI-distributed extension files.

### Btrfs-backed managed store

On Btrfs-backed storage, the store could use a pair of subvolumes, one per slot,
to simplify inactive-slot replacement without changing paths or the public API.

### Other platforms

Non-ACL backends can reuse the policy once validated against their storage and
activation models.

A future `managed` backend could prepare links in writable A/B extension search
directories during staging. Switching OS slots then selects those links without
a loader. The owned images may live on shared or slotted storage, but the links
must still resolve after reboot.

Choose separately for sysexts and confexts. Native confext integration can use
slotted links or boot-selected `/run/confexts` entries if it preserves the
platform's `/etc` mutability and user-state persistence. Prefer existing
writable A/B directories in a fixed, platform-defined order.
