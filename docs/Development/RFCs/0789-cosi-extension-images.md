# 0789 COSI Extension Images

- Date: 2026-10-08
- RFC PR: [microsoft/trident#789](https://github.com/microsoft/trident/pull/789)
- Issue: [microsoft/trident#0000](https://github.com/microsoft/trident/issues/0000)

## Summary

COSI files cannot carry sysext or confext images. This RFC stores them as
ZSTD-compressed members of the COSI tar and adds an optional `extensions`
section to the metadata. Each entry reuses the existing
[`ImageFile`](../../Reference/Composable-OS-Image.md#imagefile-object) object and
an optional destination path, mirroring the
[`Extension`](../../Reference/Host-Configuration/API-Reference/Extension.md)
object in the Host Configuration.

## Motivation and Goals

Extensions are configured through `os.sysexts` and `os.confexts`, each entry a
URL and a SHA-384. That makes an extension a second artefact: separately hosted,
fetched from a second endpoint at deploy time, with integrity anchored outside
`os.image.sha384`, and updated by a Runtime Update rather than the A/B update
that ships the OS. Content that must land with a new OS needs two operations.

Bundled, an extension is delivered and activated in the same A/B update and
reboot as the OS, reusing the existing rollout, health-gating and rollback
machinery. See [Rollback](#rollback).

### Why Not Place the Files in the Root Filesystem Image

The builder could place `my-tool.raw` in `/var/lib/extensions/` before the COSI
is built. That works in some configurations, but:

1. Under root or usr verity, modifying the protected filesystem changes the root
   hash and requires re-signing.
2. Extension destinations are commonly on a separate volume, which may be
   created empty rather than written from a partition image.
3. Filesystem image contents are invisible to tooling reading COSI metadata.
4. Trident enables the merge units, validates placement and reads
   `extension-release` only for extensions it knows about.

### Goals

- Deploy extensions from a COSI with no second artefact, endpoint or update.
- Reuse existing metadata objects and the shape of the Host Configuration API.
- Leave the Host Configuration API unchanged.

## Scope

### Requirements

- An optional `extensions` object in the metadata root, holding `sysexts` and
  `confexts` arrays.
- Each entry identifies its payload with an `ImageFile` and may specify a
  destination.
- Payloads are ZSTD-compressed tar members under the existing compression and
  integrity rules.
- Trident deploys bundled extensions on Clean Install and A/B Update.
- A change confined to the extension set is applicable as a Runtime Update. See
  [Extension-Only Updates](#extension-only-updates).
- Bundled and Host Configuration extensions coexist; conflicts are an error.

### Out of Scope

- Changes to the Host Configuration `Extension` object.
- Extension-only COSI files, carrying extensions and no partition images.
- Bundled extensions under [disk streaming](../../Explanation/Disk-Streaming.md).
  Streaming such a COSI is refused.
- Signing beyond the SHA-384 chain COSI already provides.
- SELinux compatibility, unchanged. See [SELinux](#selinux).
- Producing the DDIs.

### Exit Criteria

- COSI revision 1.3 published, with schema and samples.
- A COSI carrying a sysext and a confext deploys on Clean Install and A/B
  Update, both merged after reboot; streaming it is refused.
- A/B rollback restores the previous extension set with no extra servicing.
- An extension-only change applies as a Runtime Update, and is refused when
  anything else differs.
- Conflicts between bundled and Host Configuration extensions are a structured
  error.

## Dependencies

A COSI writer that emits the section.
[Image Customizer](https://github.com/microsoft/azure-linux-image-tools) is the
reference writer.

[Extension-Only Updates](#extension-only-updates) also require:

- A writer mode copying an existing COSI's region images verbatim while
  replacing its extension set. Without it, every extension change is an A/B
  update.
- A surface for requesting a servicing type. None exists; `forceAbUpdate` is the
  nearest precedent. It must define precedence against `forceAbUpdate` and
  whether the request survives separate stage and finalize invocations.

Three existing defects must be fixed first. All are reachable today; bundling
makes the first two routine.

1. **In-place replacement deletes the new image.** When an extension keeps its
   ID and destination but changes content, `set_up_extensions` schedules the ID
   for both addition and removal. The addition renames over the destination; the
   removal then deletes that path, because the old entry's `temp_path` is its
   destination. The guard assumes the paths differ.
2. **Staged payloads do not survive the operation.** Runtime finalize and
   rollback build their `EngineContext` with `image: None`, so the COSI is gone
   after stage.
3. **Placement is validated against the Host Configuration only, before the COSI
   is read.** See [Destination Validation](#destination-validation).

## Implementation

### Tar Layout

Payloads are ZSTD-compressed DDIs under `images/extensions/`.

They stay under `images/`, which the specification already permits and requires
readers to handle. Relaxing `ImageFile.path`'s `^images/.+` pattern would
produce a 1.3 `ImageFile` that fails the 1.0–1.2 schemas, for a shorter path.

The primary GPT image must immediately follow `metadata.json` since revision
1.2, so payloads must not sit between them. Payloads are not regions and do not
participate in region ordering, but should be written after all region images to
keep that ordering verifiable and preserve sparse-read locality. Writers must
account for them in `compression.maxWindowLog`.

Older readers are unaffected: unknown files must be ignored, and Trident's
orphan-image check (`V1_2ImageFileHasNoCorrespondingPartition`) reads the
`images[]` and `disk.gptRegions[]` arrays rather than walking tar entries.

### Metadata Schema

| Field        | Type                             | Added in | Required | Description                     |
| ------------ | -------------------------------- | -------- | -------- | ------------------------------- |
| `extensions` | [Extensions](#extensions-object) | 1.3      | No       | Extension images in this COSI.  |

#### `Extensions` Object

| Field      | Type                                       | Added in | Required | Description                     |
| ---------- | ------------------------------------------ | -------- | -------- | ------------------------------- |
| `sysexts`  | [ExtensionImage](#extensionimage-object)[] | 1.3      | No       | System extension images.        |
| `confexts` | [ExtensionImage](#extensionimage-object)[] | 1.3      | No       | Configuration extension images. |

#### `ExtensionImage` Object

| Field   | Type                                                                 | Added in | Required        | Description                                     |
| ------- | -------------------------------------------------------------------- | -------- | --------------- | ----------------------------------------------- |
| `image` | [ImageFile](../../Reference/Composable-OS-Image.md#imagefile-object) | 1.3      | Yes (since 1.3) | The compressed extension image in the tar file. |
| `path`  | string                                                                | 1.3      | No              | Absolute destination path on the target OS.     |

```json
{
  "properties": {
    "extensions": {
      "description": "Extension images carried by this COSI file.",
      "$ref": "#/$defs/Extensions"
    }
  },
  "$defs": {
    "Extensions": {
      "type": "object",
      "properties": {
        "sysexts": {
          "description": "System extension images to place on the target OS.",
          "type": "array",
          "items": { "$ref": "#/$defs/ExtensionImage" }
        },
        "confexts": {
          "description": "Configuration extension images to place on the target OS.",
          "type": "array",
          "items": { "$ref": "#/$defs/ExtensionImage" }
        }
      }
    },
    "ExtensionImage": {
      "type": "object",
      "required": ["image"],
      "properties": {
        "image": {
          "description": "Details of the compressed extension image file in the tar file.",
          "$ref": "#/$defs/ImageFile"
        },
        "path": {
          "description": "Absolute path of the extension image on the target OS. The file name MUST be `{name}.raw`, where `{name}` matches the suffix of the `extension-release.{name}` file inside the image. When omitted, the reader places the image in its default directory for the extension kind.",
          "type": "string",
          "pattern": "^/.+\\.raw$"
        }
      }
    }
  }
}
```

Example:

```json
"extensions": {
  "sysexts": [
    {
      "image": {
        "path": "images/extensions/gpu-driver.rawzst",
        "compressedSize": 41943040,
        "uncompressedSize": 134217728,
        "sha384": "3a1f9c0d4e5b6a7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e"
      },
      "path": "/var/lib/extensions/gpu-driver.raw"
    }
  ],
  "confexts": [
    {
      "image": {
        "path": "images/extensions/fleet-config.rawzst",
        "compressedSize": 262144,
        "uncompressedSize": 1048576,
        "sha384": "c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2"
      }
    }
  ]
}
```

#### Two Arrays Rather Than One With a `kind` Field

Sysexts and confexts have disjoint permitted directories, different defaults,
different `extension-release` locations, different identity fields and different
activation units. Every downstream rule branches on the kind. Two arrays make it
structural, and match the Host Configuration.

#### Entry Contents

`image` and an optional `path`. Everything else is derivable from the payload,
and each derivable field is a second source of truth.

- **`path`** is kept: the destination is not derivable. Optional, defaulting as
  `Extension.path` does. Existing rules apply — absolute, ending in `.raw`, in a
  permitted directory for its kind, file name matching the `extension-release`
  suffix.
- **Name** is rejected: derived from `extension-release.{name}`, which the
  destination file name must already match.
- **`kind`** is rejected: structural.
- **Extension ID** is rejected: read from `extension-release`, and already the
  identity Trident keys A/B state on.
- **Enable-on-first-boot** is rejected: systemd merges everything in the
  extension directories, and the Host Configuration has no equivalent switch.
- **Version and compatibility metadata** is rejected: already in the DDI and
  enforced by systemd. Copying it would let a COSI claim compatibility the
  payload lacks.

#### `sha384` Semantics

`ImageFile.sha384` covers the compressed image; `Extension.sha384` covers the
raw file. For integrity this matches partition images, since Trident hashes the
compressed stream as it decompresses.

`ExtensionData.sha384` serves both as change detector and as the key matching a
processed extension to its Host Configuration entry, so entries must record the
hash with its source and compare only within a source. Across sources the
comparison is always unequal and would report every extension as changed.

Within a source it is sound but imprecise: compressed hashes are unstable across
recompressions, so identical DDIs at different ZSTD levels appear to differ. The
result is a redundant copy, never a missed update.

### Versioning

COSI revision 1.3. `extensions` is optional; absent equals empty. A newer
Trident reading a 1.0–1.2 COSI behaves as today.

An older Trident reading a 1.3 COSI accepts it, since the version check rejects
only `major != 1` and unknown fields must be ignored. It deploys the OS and
omits the extensions silently. This cannot be corrected in-band: any tripwire
field is one such a reader must ignore. Nor does it self-heal — the old reader
records the new COSI as deployed, so after upgrading Trident the same Host
Configuration is no longer an image change. Recovery needs a forced
redeployment.

Mitigations: document the requirement; warn in
`validate_cosi_metadata_version` when `minor` exceeds the highest known
revision; and, where the target's Trident version cannot be controlled, gate the
update on a
[health check](../../Reference/Host-Configuration/API-Reference/Health.md)
asserting the extension is merged. Given that the omission does not self-heal,
the health check is the only real safeguard.

### Trident-Side Consumption

Trident computes an **effective extension set**, the union of the Host
Configuration and COSI entries, and the code reading `ctx.spec.os.sysexts` and
`confexts` reads that instead. Each entry carries its source. Three consumers:

- **`ExtensionsSubsystem`.** For a bundled entry only the source changes: the
  image streaming pipeline decompresses the tar member into the staging
  directory and verifies `ImageFile.sha384` over the compressed stream, in place
  of the `FileReader` download. `update_host_configuration` fails when a
  processed extension has no Host Configuration entry matching by hash, so it
  must write back resolved paths for Host-Configuration-sourced entries only.
- **`osconfig`.** The merge units are enabled only when `ctx.spec.os.sysexts` or
  `confexts` are non-empty, so a COSI-only host would never have them enabled.
- **`selinux`.** The validation raising `ExtensionImagesAndSelinuxUnsupported`
  must also read the effective set, but that alone is insufficient: it runs
  inside `configure`, which returns early for Runtime Updates, and
  `SelinuxSubsystem` inherits `runs_on = REQUIRES_REBOOT`. It must move to a
  hook running on every servicing type, or
  [Extension-Only Updates](#extension-only-updates) bypass it.

[Disk streaming](../../Explanation/Disk-Streaming.md) is excluded: `osconfig`
returns early when `is_stream_image` is set, so the units are never enabled and
the extensions would be placed and never merged. Trident refuses to stream a
COSI carrying extensions rather than produce that state.

#### Deployed Extension Inventory

The Host Configuration records what an operator asked for, so `ctx.spec_old`
does not describe what is deployed. Without a second record Trident cannot tell
which bundled extensions to remove, detect collisions against the deployed set,
compare sets for servicing selection, or rebuild its state when finalize runs
separately from stage.

Trident records a deployed extension inventory in the Host Status, alongside the
[deployed image summary](#deployed-image-summary): kind, extension ID, name,
resolved destination, source, and the hash under that source's semantics.
`extensions_old` is populated from it.

Inventory and summary follow the discipline applied to `spec` and `spec_old`: a
staged operation records a pending record while the deployed one is retained.
Promotion follows the servicing type, not the finalize step. Runtime Update and
`ManualRollbackRuntime` promote on completion. Clean Install, A/B Update and
manual A/B rollback reach `*Finalized` before rebooting and commit after booting
the expected root, so their pending record must survive the reboot and be
discarded when the boot check fails.

`ManualRollbackChainItem` carries only kind, `spec`, active volume and install
index, so it must carry the historical inventory and summary too. Even then a
manual runtime rollback has no payload: its context is built with `image: None`
and the superseded image is retained only for the operation that replaced it.
Either payloads are kept in a content-addressed store for as long as the
rollback chain references them, or that rollback is refused. Refusing is the
safe default.

#### Destination Validation

`validate_extension_images_locations` rejects destinations not on an A/B volume
when A/B is configured, but it is static validation over the Host Configuration
lists and runs before the COSI is read.

The equivalent check for the effective set runs after the COSI loads, against
the storage graph. It must resolve the filesystem that actually backs the
destination for the servicing type: under root-verity the engine mounts a
writable `/etc` overlay during provision, backed by `/var/lib/trident-overlay`
on an A/B volume, so `/etc/extensions/` is writable and rolls back with the
slot. Rejecting verity-backed destinations outright would reject that case. The
same destination has no such backing on a Runtime Update.

#### Where Images Are Written

On Clean Install and A/B Update, `provision()` runs with the target root at
`mount_path` and extensions are placed inside it — for A/B, the inactive slot.
Staging currently uses a fixed `/var/lib/extensions/.staging` with a non-atomic
copy when the rename crosses a filesystem boundary; it should use a temporary
file on the destination's own filesystem.

On Runtime Update `provision()` is not called, so partitions are never touched.

#### Rollback

Extension images are files in the slot's own filesystem, so where every
destination is on an A/B volume the previous slot retains the previous set and
an A/B rollback reverts it with the OS.

That depends on the placement check covering bundled destinations; see
[Destination Validation](#destination-validation). A bundled extension on a
shared volume changes the running slot immediately and cannot be rolled back.

#### Extension-Only Updates

`ab_update_required()` returns true whenever `os.image.sha384` differs, before
any subsystem is consulted, and the metadata hash covers the `extensions`
section. Changing only a bundled extension would force a full A/B update,
making bundling strictly more disruptive than the status quo.

An extension-only COSI is the wrong shape for this. The metadata root requires
`images`, `disk` with at least one `gptRegions` entry, `bootloader` and
`osPackages`, and every consumer of `os.image` assumes a complete OS. The
distinction is drawn on content equality, not absent content.

##### Deployed Image Summary

Trident retains only the URL and metadata hash of the applied image, and
re-fetching the previous COSI to diff it is not dependable, so it must record
what it deployed.

The summary holds, per entry in `images[]` and `disk.gptRegions[]`, the
`image.sha384`, `uncompressedSize` and identity (partition number, mount point,
`fsType`, `fsUuid`, `partType`, verity root hash); plus `osArch`, `osRelease`,
`disk` geometry and `bootloader`. It excludes `extensions`, the subject of the
comparison; `osPackages`, which Trident validates but never acts on;
`compression.maxWindowLog`, which a new extension may legitimately raise; and
`id`, which identifies the file rather than its content.

A record rather than a digest, so Trident can report which partition differs.

The summary is versioned. Where absent or unrecognised, Trident falls back to
comparing `os.image.sha384`, so existing hosts are unaffected until their next
deployment.

| Summary | Extension set | Image requires    |
| ------- | ------------- | ----------------- |
| Equal   | Equal         | Nothing.          |
| Equal   | Differs       | A Runtime Update. |
| Differs | Any           | An A/B update.    |

This is the image's requirement, not the outcome; `select_servicing_type` still
takes the maximum across subsystems. The no-op row is new, since
`os.image.sha384` changes whenever any part of the metadata does.

##### Constructing an Extension-Only Update

Summary equality requires byte-identical region images, and filesystem images
are not reproducible: UUIDs, inode timestamps, superblock times and allocation
order vary between builds, and ZSTD output varies with level and library
version.

So the build operation is not "rebuild the image identically with a different
extension set", it is "copy an existing COSI, replacing its extension set".
Region images and their metadata entries are copied verbatim; only the extension
members, the `extensions` section, `compression.maxWindowLog` and `id` change.
Equality then holds by construction, and the operation is a tar rewrite.

A COSI rebuilt from source will differ and Trident will select an A/B update.
The failure mode of an unreproducible build is a redundant A/B update, never a
skipped one.

##### Requested and Verified, Never Inferred

An extension-only update proceeds when all of:

1. A Runtime Update is explicitly requested.
2. The recorded summary equals the one computed from the new COSI.
3. The recorded image is the deployed one: servicing completed, no A/B update
   pending, active volume matching the Host Status.
4. No other subsystem requires an A/B update.

If 2, 3 or 4 fails, Trident fails with a structured error naming what differs.
It must not promote the operation to an A/B update, nor apply the extension
change while leaving other changes unapplied.

A no-op must still record the accepted image and summary.
`select_servicing_type` returning `NoActiveServicing` makes `update` return
without persisting anything, so a metadata-only status transition is needed.

##### Rollback on This Path

A Runtime Update replaces files in the running slot, because
`set_up_extensions` skips removal of the superseded image only on Clean Install
and A/B Update. Rollback re-runs the subsystem with the specs reversed and must
restore the previous image, but cannot re-fetch it: finalize and rollback build
their context with `image: None`, and auto-rollback runs unconditionally after a
finalize failure. The superseded image must be retained for the operation.

Rollback is therefore bounded by that retained payload rather than by booting an
untouched slot, which is why the path is requested rather than inferred.

#### Interaction With Host Configuration `sysexts` and `confexts`

The sets are merged, and a collision is an error.

Both are valid at once: bundled extensions are content the image author
considers part of the OS, Host Configuration entries are content the operator
adds. Silent precedence is rejected, since an override would run software the
image author never validated with no visible symptom.

A collision is either the same destination path, detectable from metadata when
both sides specify `path`, or the same extension ID within a kind, detectable
only after mounting. ID uniqueness is documented but unenforced today, so it
should become an enforced check.

Two entries identical in every respect are still a collision; the operator can
drop theirs. A per-extension override, if needed, belongs on the Host
Configuration side as an opt-in. See [Open Questions](#open-questions).

### Validation

#### COSI Metadata Validation

New `CosiMetadataErrorKind` variants, following the existing `V1_<minor>`
prefix:

| Variant                                           | Condition                                                                                       |
| ------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `V1_3ExtensionDestinationPathNotAbsolute`          | `path` is present and not absolute.                                                              |
| `V1_3ExtensionDestinationPathInvalidFileExtension` | `path` is present and does not end in `.raw`.                                                    |
| `V1_3ExtensionDestinationPathInvalidDirectory`     | `path`'s parent is not in `VALID_SYSEXT_DIRECTORIES` or `VALID_CONFEXT_DIRECTORIES` for the kind. |
| `V1_3DuplicateExtensionDestinationPath`            | Two entries both specify `path` and specify the same one.                                        |
| `V1_3DuplicateExtensionImagePath`                  | Two entries reference the same tar member.                                                       |
| `V1_3ExtensionImagePathCollidesWithRegionImage`    | An entry's `image.path` is also referenced by `images[]` or `disk.gptRegions[]`.                  |

These reuse the logic behind `Extension::validate_sysext` and
`validate_confext`.

Destination collisions are only partly detectable here, since an entry omitting
`path` resolves using the `{name}` inside the DDI. Member existence is not
checked at load either: `Cosi::new` scans only as far as `metadata.json` and
discovers the rest on demand, and checking up front would conflict with sparse
reads.

#### Deploy-Time Validation

After the DDI is mounted. The first two are already enforced by
`read_extension_release` and apply unchanged.

- File name matches the `extension-release` suffix.
- Exactly one `extension-release` file, with `SYSEXT_ID` or `CONFEXT_ID`.
- SHA-384 agreement, verified over the compressed stream during decompression.
- Resolved destination collisions across the effective set.
- Extension ID uniqueness across the effective set, per kind.
- Destination placement. See [Destination Validation](#destination-validation).
- `extension-release` OS compatibility: warn when a bundled extension declares
  an `ID=<distro>` not matching the COSI's `osRelease`, since systemd will refuse
  to merge it. Warn on `ID` alone — systemd accepts a `VERSION_ID` mismatch when
  `SYSEXT_LEVEL` matches, so warning on version would flag valid extensions.

Trident should not warn or refuse on `ID=_any`. It describes what the extension
is compatible with, not how it should be delivered, and bundling a portable
extension is how an air-gapped deployment is achieved.

### SELinux

Unchanged. Extensions remain incompatible with SELinux in enforcing mode on
systemd 255, because merging the overlays mislabels `/usr`, `/opt` and `/etc`.

What changes is where the check looks and when it runs: it inspects the Host
Configuration lists only, and runs inside `configure`, which returns early for
anything but Clean Install and A/B Update.

## Public API Design

The COSI metadata format is a public contract with its own
[specification](../../Reference/Composable-OS-Image.md) and published schemas, so
the additions above are the public API change. They are additive and optional.

The Host Configuration API does not change. A host may now end up with more
extensions than it lists, and specifying one the image already carries is an
error.

## Testing and Metrics

- **Schema.** Add `cosi-metadata-v1.3.schema.json` and samples under
  `tests/cosi/metadata_samples/v1.3/`; the existing workflow picks up a new
  revision automatically. Invalid samples cover what the schema can express:
  `image.path` outside `images/`, a non-absolute destination, one not ending in
  `.raw`. The permitted-directory allow-list is Trident policy, not a property
  of the format, so it stays out of the schema and is tested in Rust.
- **Unit.** Each new error variant; effective set computation including
  collisions and the source discriminator; default-path resolution; replacement
  of an extension keeping its ID and destination but changing content.
- **Functional.** Clean Install of a COSI carrying a sysext and a confext places
  both, enables the merge units, and both merge after boot.
- **Servicing.** A/B update from extension set A to B, with rollback restoring
  A. An extension-only Runtime Update, asserting partitions are untouched. Its
  finalize and rollback as distinct processes, asserting the previous image is
  restored without access to either COSI. A pending inventory surviving an A/B
  reboot and discarded when the boot check fails.
- **Negative.** Streaming a COSI carrying extensions; the same extension in both
  sources; a bundled extension with SELinux `enforcing`, on both A/B and
  extension-only Runtime Update; a bundled destination on a shared volume; a
  Runtime Update between differing image summaries.

## Servicing

Additive at every layer. A 1.2 COSI, and a 1.3 COSI without `extensions`, behave
identically. A 1.3 COSI with extensions read by an older Trident deploys the OS
and omits them; see [Versioning](#versioning). Existing hosts have no recorded
image summary, so servicing type selection is unchanged until their next
deployment.

## Implementation Plan

0. Prerequisites from [Dependencies](#dependencies). The in-place replacement
   fix is a standalone bug and should land independently.
1. COSI revision 1.3: the `extensions` section, schema and samples.
2. Trident reader: parse and validate `extensions`, plus the minor-version
   warning.
3. Effective extension set and deployed extension inventory, plumbed into the
   extensions, `osconfig` and `selinux` subsystems.
4. Stream a tar member into the staging directory; dynamic destination
   validation.
5. Deployed image summary and the extension-only Runtime Update path.
6. Tests, then documentation updates to
   [Sysexts](../../Explanation/Sysexts.md),
   [Confexts](../../Explanation/Confexts.md) and
   [How Trident Consumes COSI](../../Explanation/How-Trident-Consumes-COSI.md).

Steps 1 and 2 are independently useful, and steps 3 and 4 are shippable without
step 5. Step 5 is still required for this RFC to be complete, since without it
every extension change is an A/B update.

## Counter-Arguments

### Drawbacks

- **Cadence coupling.** A new version of the extension requires a new COSI.
  [Extension-Only Updates](#extension-only-updates) remove the reboot and reduce
  the build to a copy, but it must still be published. Free for content that
  must match the OS; a real loss of agility for content versioned independently,
  where the Host Configuration route stays correct.
- **Size.** Every host pays for every bundled extension, including unused ones.
- **No opt-out.** A host wanting the image but not one of its extensions cannot
  say so.
- **Two sources.** Merging and its collision rules add complexity.
- **Forward compatibility.** An older reader omits the extensions silently.

### Alternatives

**Host Configuration only (status quo).** Maximum decoupling, and a change is a
Runtime Update with no reboot. Remains supported, and is wrong only when the
extension must land in lockstep with a new OS.

**Place the `.raw` files in the root filesystem image.** See
[above](#why-not-place-the-files-in-the-root-filesystem-image).

**Relax `ImageFile.path` to allow a top-level `extensions/` prefix.** See
[Tar Layout](#tar-layout).

**One array with a `kind` discriminator.** See
[above](#two-arrays-rather-than-one-with-a-kind-field).

**Prior art.** Flatcar's sysext-bakery pairs pre-built DDIs with
[`systemd-sysupdate`](https://www.freedesktop.org/software/systemd/man/latest/systemd-sysupdate.html),
hosting artefacts over HTTP with a `SHA256SUMS` manifest. Like `os.sysexts`, it
optimises for extensions that move independently of the OS, though without an
OCI transport. Bundling optimises for extensions that must move with it.

## Open Questions

- **What does the deployed image summary cover?** Excluding too much permits a
  Runtime Update between images differing in ways Trident does not model.
  `version` is currently excluded.
- **A structured record or a digest?** A record costs more but names the
  partition that differs.
- **Does the no-op outcome belong here?** It needs a metadata-only status
  transition, which is a change independent of extensions.
- **Where does the retained superseded image live, and for how long?** The
  staging directory covers only the operation; a content-addressed store bounded
  by the rollback chain also covers manual runtime rollback, at the cost of
  disk.
- **Should an override escape hatch exist?** It would permit pinning a hotfixed
  extension, and equally permit a combination the image author did not validate.
  Recommendation: ship strict.
- **Should a bundled extension be suppressible?** No analogue in the current API
  and no concrete requester.
- **Should `path` be required?** Optional is proposed for symmetry with
  `Extension.path`.
- **Naming.** `extensions` containing both arrays, against two top-level arrays.
- **Should Host Configuration extension ID uniqueness be enforced separately?**
  It is a pre-existing gap this RFC depends on.
- **Must writers place payloads after all region images, or is SHOULD enough?**

## Future Possibilities

- **Portable service images**, DDIs with the same shape and placement problem.
- **Per-extension selection at deploy time**, keyed on extension ID.
- **Initrd-scoped extensions.** `SYSEXT_SCOPE=initrd` is parsed but not acted
  on, and a bundled payload is available before the root filesystem.
- **Bundled extensions under disk streaming**, needing a way to enable the merge
  units on a streamed install.
- **Reporting the extension inventory** through the CLI and gRPC API.
