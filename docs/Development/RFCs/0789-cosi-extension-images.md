# 0789 COSI Extension Images

- Date: 2026-09-08
- RFC PR: [microsoft/trident#789](https://github.com/microsoft/trident/pull/789)

## Summary

Carry sysext and confext images as compressed members of a COSI tar. An optional
`extensions` section lists each [`ImageFile`](../../Reference/Composable-OS-Image.md#imagefile-object)
and, when placement is prescribed, its destination `path`. Trident can deploy
extensions with the OS image instead of fetching them separately.

## Motivation and Goals

Host Configuration extensions need separate hosting, downloads and updates.
Bundling gives OS-coupled extensions the COSI's integrity chain and the same
A/B rollout, reboot and rollback as the OS. Host Configuration extensions remain
available for content released independently.

## Scope

### Requirements

- COSI revision 1.3 adds optional `extensions.sysexts` and
  `extensions.confexts` arrays, each entry `{ id, image, path? }`.
- Clean Install and A/B Update deploy the effective set from COSI and Host
  Configuration; collisions fail.
- An extension-only change can use Runtime Update without rewriting partitions
  or rebooting.

### Out of Scope

Changing the Host Configuration API, DDIs, SELinux support or signing;
partition-less COSI files; and bundled extensions under
[disk streaming](../../Explanation/Disk-Streaming.md), which must be refused
until streaming can enable the merge units.

### Exit Criteria

Publish the 1.3 schema and samples. Deploy sysexts and confexts on Clean Install
and A/B Update with slot-correct rollback; apply a verified extension-only
change by Runtime Update. Reject collisions and unsupported disk streaming.

## Dependencies

[Image Customizer](https://github.com/microsoft/azure-linux-image-tools), or
another writer, must emit the section. Extension-only updates also need a
COSI-to-COSI writer that copies region images verbatim and an explicit
Runtime Update request (including its precedence over `forceAbUpdate`).

Fix the existing `set_up_extensions` replacement bug first: with the same ID
and destination but different content, it installs the new file then deletes
that path as the old file. Runtime stage/finalize also needs durable payloads;
finalize and rollback construct their context with `image: None`.

## Implementation

### Tar Layout

Payloads are ZSTD-compressed DDIs under `images/extensions/`; this preserves
`ImageFile.path`'s `^images/.+` constraint. The primary GPT image must
immediately follow `metadata.json`. Write extension members after the
disk-region images and include their window sizes in `compression.maxWindowLog`.
Older readers ignore the additional tar members.

### Metadata Schema

The two arrays mirror Host Configuration and determine the kind; each entry
contains an `id`, an `image` and optional `path`. This is the proposed addition to
the [1.2 schema](../../Reference/Composable-OS-Image/cosi-metadata-v1.2.schema.json):

```json
{
  "properties": {
    "extensions": { "$ref": "#/$defs/Extensions" }
  },
  "$defs": {
    "Extensions": {
      "type": "object",
      "properties": {
        "sysexts": { "type": "array", "items": { "$ref": "#/$defs/ExtensionImage" } },
        "confexts": { "type": "array", "items": { "$ref": "#/$defs/ExtensionImage" } }
      }
    },
    "ExtensionImage": {
      "type": "object",
      "required": ["id", "image"],
      "properties": {
        "id": { "type": "string", "minLength": 1 },
        "image": { "$ref": "#/$defs/ImageFile" },
        "path": { "type": "string", "pattern": "^/.+\\.raw$" }
      }
    }
  }
}
```

Example (`extensions` fragment of `metadata.json`):

```json
{
  "sysexts": [{
    "id": "gpu-driver",
    "image": {
      "path": "images/extensions/gpu-driver.rawzst",
      "compressedSize": 41943040,
      "uncompressedSize": 134217728,
      "sha384": "3a1f9c0d4e5b6a7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e"
    }
  }],
  "confexts": [{
    "id": "fleet-config",
    "image": {
      "path": "images/extensions/fleet-config.rawzst",
      "compressedSize": 262144,
      "uncompressedSize": 1048576,
      "sha384": "c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2"
    },
    "path": "/var/lib/confexts/fleet-config.raw"
  }]
}
```

`id` is the extension's stable logical identity, not the COSI root `id`, the
host OS's `ID=`, or the image filename. For a sysext it must equal `SYSEXT_ID=` inside
`/usr/lib/extension-release.d/extension-release.{name}`; for a confext it
must equal `CONFEXT_ID=` inside
`/etc/extension-release.d/extension-release.{name}`. Trident already requires
those fields in DDIs. `(kind, id)` identifies an extension across COSI versions;
its compressed hash triggers replacement. IDs must be unique within each
array. Metadata can now match old and new extensions before reading their
payloads, but Trident must still verify the declared ID against the DDI.

`ImageFile` supplies the tar path, compressed size for ranged reads,
uncompressed size for output bounds, and SHA-384 of the compressed member.
Trident checks both sizes and the hash against the bytes read and written.
It does not reuse Host Configuration's `Extension.sha384`, which hashes the
uncompressed DDI; a source switch must replace the image, not compare those
two hashes. Recompression may cause an unnecessary replacement, never a missed
one.

`path` is deliberately optional, **not** a missing piece of metadata. Omission
leaves placement to Trident and may be the usual case for fully
Trident-managed extensions. Today Trident derives `{name}.raw` from
`extension-release.{name}` and uses `/var/lib/extensions/` or
`/var/lib/confexts/`. An explicit `path` must use a permitted directory and
match that name. [RFC 0838](https://github.com/microsoft/trident/pull/838)
proposes fully Trident-managed, slotted sysext storage without `path`; it
handles `/etc` layers separately. How bundled extensions select such a policy
is unresolved; omitting `path` alone must not opt in.

There is no `url` (the tar is the source), `kind` (the array supplies it), or
separate name, enable flag or compatibility fields (the DDI supplies them, or
systemd merges all discovered images). The name determines the required
`{name}.raw` filename and can affect systemd's merge precedence; it is not
Trident's update identity.

### Versioning

COSI revision 1.3 adds the optional section; older COSIs behave unchanged.
Existing Trident accepts any `1.x` metadata and ignores unknown fields, so it
silently skips bundled extensions. That omission does not self-heal after a
Trident upgrade because the image is already recorded as deployed. Require a
1.3-aware reader, warn on unknown minor versions in new readers, and use a
[health check](../../Reference/Host-Configuration/API-Reference/Health.md) for
required extensions when reader versions cannot be controlled.

### Trident-Side Consumption

Compute an effective set from COSI and Host Configuration entries, retaining
each entry's source. Match old and new entries by `(kind, id)`; replace when
content changes, and reconcile changes to placement or filename. The extensions
subsystem reads bundled members instead of downloading them;
`update_host_configuration` writes resolved paths back only for Host
Configuration entries. `osconfig` enables the sysext/confext merge
units for the effective set. Move the SELinux check into validation that also
runs for Runtime Updates; its current `configure` hook does not.

[Disk streaming](../../Explanation/Disk-Streaming.md) must reject bundled
extensions: `osconfig` skips stream-image configuration, leaving the merge
units disabled.

#### Deployed Extension Inventory

Host Configuration does not record bundled extensions. Persist an inventory
(kind, ID, name, resolved placement, source and source hash) in Host Status so
Trident can compare old/new sets and resume separate stage/finalize calls.
Retain deployed and pending inventories and image summaries alongside
`spec_old` and `spec`. Promote after successful Runtime Update, or after
post-boot commit for A/B; discard pending data on rollback. Extend
`ManualRollbackChainItem` to carry historical inventory and summary.

Manual runtime rollback also needs the old DDI bytes. Either retain them while
the rollback chain references them or refuse rollback across a bundled
extension change; metadata alone cannot restore them.

#### Destination Validation

Existing A/B location validation reads only Host Configuration entries before
loading COSI. Validate the *resolved* effective set after reading the image:
every destination must be writable and, when A/B is configured, slot-scoped.
A/B volumes qualify; the root-verity `/etc` overlay qualifies during
provisioning if its backing volume is A/B. Slotted storage from
[RFC 0838](https://github.com/microsoft/trident/pull/838) qualifies only with
its loader capability; `path` omission alone must not bypass the A/B check.

#### Where Images Are Written

Clean Install and A/B Update place files in the target root (the inactive slot
for A/B). Runtime Update places them in the running root without provisioning
partitions. Stage temporary files on the destination filesystem for atomic
placement; the current fixed `/var/lib/extensions/.staging` falls back to a
non-atomic cross-filesystem copy.

#### Rollback

An A/B rollback restores the previous extension set only when placement is
slot-scoped. Writing a bundled image to a shared volume would change the running
slot and defeat rollback; see [Destination Validation](#destination-validation).

#### Extension-Only Updates

Today any change to the COSI metadata hash forces A/B servicing, including an
extension-only change. A partition-less COSI is not an option: `os.image`
describes a complete OS. Compare the new COSI against a stored summary of the
deployed OS instead.

##### Deployed Image Summary

Store each region's hash, uncompressed size and identity (partition number,
mount point, `fsType`, `fsUuid`, `partType`, verity root hash), plus disk geometry,
`osArch`, `osRelease` and `bootloader`. Exclude extensions, packages, compression
settings and `id`: none changes the deployed OS. A structured summary identifies
which region differs. If missing or of an unknown summary version, fall back to
the existing metadata-hash comparison.

Equal summaries and equal extension sets require no servicing; equal summaries
with changed extensions permit Runtime Update; a changed summary requires A/B.
Other subsystems can still require A/B. A no-op needs a metadata-only Host Status
update, since `update` currently returns without recording the accepted image.

##### Constructing an Extension-Only Update

Copy an existing COSI's region images and metadata entries verbatim; replace
only the extension members and metadata (and `compression.maxWindowLog` or `id`
if needed). Rebuilding filesystems changes bytes and hashes, even for the same
files, and safely selects A/B instead of skipping an OS change.

##### Requested and Verified, Never Inferred

Runtime Update requires an explicit request, matching OS summaries, a
committed/active source slot, and no A/B requirement from any other subsystem.
If any condition fails, report an error; do not partially apply changes or
silently switch to A/B.

##### Rollback on This Path

Runtime Update changes the running slot. Retain the old DDI until finalize and
auto-rollback complete; both can run with `image: None`. Historical manual
rollback needs longer retention or must be refused.

#### Interaction With Host Configuration `sysexts` and `confexts`

Merge both sources. Reject duplicate extension IDs per kind or destination
collisions after resolving placement, including omitted `path`. Silent
precedence could run an unvalidated combination; identical entries still
collide. COSI ID duplicates can be rejected from metadata; Host Configuration
IDs still require reading their DDIs. Enforce the Host Configuration's
documented ID-uniqueness rule too.

### Validation

In `cosi/validation.rs`, add `V1_3` error kinds for invalid explicit paths
(absolute, `.raw`, allowed directory), duplicate `(kind, id)` pairs, duplicate
explicit destinations, reused tar members and collisions with disk-region
images. Reuse
`Extension::validate_sysext`/`validate_confext` path rules. A missing member is
detected when read: `Cosi::new` only scans as far as `metadata.json`.

After decompressing and mounting the DDI, check actual compressed and
uncompressed sizes, the compressed SHA-384, exactly one `extension-release`
whose `SYSEXT_ID` or `CONFEXT_ID` equals the metadata `id`, the `{name}.raw`
destination suffix, resolved placement, and duplicate destinations/IDs across
both sources. Omitted paths cannot be collision-checked from metadata alone.

Warn on definite OS `ID` mismatch with COSI `osRelease`, not `VERSION_ID`:
systemd may accept a version mismatch via `SYSEXT_LEVEL` or `CONFEXT_LEVEL`.
Do not warn on `ID=_any`; it declares compatibility, not delivery method.

### SELinux

Still unsupported in enforcing mode on systemd 255. Check the effective set in
validation for every servicing type, not only the Host Configuration lists in
`configure`, which skips Runtime Updates.

## Testing and Metrics

- v1.3 schema and valid/invalid samples, including missing or empty IDs;
  semantic path and duplicate-ID rules in Rust tests.
- Clean Install and A/B Update with both kinds, merge-unit activation and
  rollback (including absent `path` and slotted placement once supported).
- Runtime Update with identical region images, distinct stage/finalize and
  auto-rollback; same-ID/same-path replacement and historical rollback refusal.
- Reject mixed-source collisions, invalid destinations, enforcing SELinux,
  mismatched metadata/DDI IDs, changed OS summaries under requested Runtime
  Update, and disk streaming.

## Implementation Plan

1. Fix [prerequisites](#dependencies); publish COSI 1.3 schema and samples.
2. Read, validate and install bundled extensions; persist the effective set.
3. Add the image summary, copy-based writer and verified Runtime Update path.

Step 2 can ship before step 3, but the RFC is not complete without both.

## Counter-Arguments

### Drawbacks

Bundling still ties distribution to a new COSI, increases its size and gives
hosts no per-extension opt-out. Use Host Configuration for extensions that must
move independently. Older readers silently omit bundled extensions.

### Alternatives

Embedding `.raw` files in a partition image hides them from COSI metadata,
fails when the destination is a separately created filesystem, and changes
verity hashes. A separate Host Configuration URL remains preferable for
independent release cycles.

Flatcar's sysext-bakery pairs pre-built DDIs with
[`systemd-sysupdate`](https://www.freedesktop.org/software/systemd/man/latest/systemd-sysupdate.html),
hosting artefacts over HTTP with a `SHA256SUMS` manifest. Like `os.sysexts`, it
optimises for extensions that move independently of the OS, though without an
OCI transport. Bundling optimises for extensions that must move with it.

## Open Questions

- **Slotted placement:** how does a COSI entry request the policy in
  [RFC 0838](https://github.com/microsoft/trident/pull/838)? Omitting `path`
  cannot silently change an existing image's placement or imply loader support.
- **Runtime request:** where is the explicit request recorded across
  stage/finalize, and how does it interact with `forceAbUpdate`?
- **Manual runtime rollback:** keep historical DDIs as long as the rollback
  chain needs them, or refuse rollback across bundled extension changes?
- **Operator override:** retain the hard error on bundled/Host Configuration
  collisions, or permit an explicit override when a hotfix is needed?
