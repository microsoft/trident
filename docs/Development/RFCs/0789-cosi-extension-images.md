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

### Out of Scope

- Changes to the Host Configuration API, DDIs, SELinux support or signing.
- The Image Customizer API and implementation for producing these COSIs;
  separate work.
- Persisting COSI extensions in Host Configuration; covered by
  [RFC 0838](https://github.com/microsoft/trident/pull/838).
- Partition-less COSI files.
- Extension-only Runtime Updates. Any change to a COSI, including only its
  extensions, requires an A/B update.
- Bundled extensions under [disk streaming](../../Explanation/Disk-Streaming.md);
  refuse them until streaming can enable the merge units.

### Exit Criteria

Publish the 1.3 schema and samples. Deploy sysexts and confexts on Clean Install
and A/B Update with slot-correct rollback. Reject collisions and unsupported
disk streaming.

## Dependencies

A separately defined [Image Customizer](https://github.com/microsoft/azure-linux-image-tools)
API must produce COSIs with embedded extensions. RFC 0838 must also persist
bundled extensions in Host Configuration for subsequent A/B updates and
rollback.

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
      "path": "images/extensions/gpu-driver.raw.zst",
      "compressedSize": 41943040,
      "uncompressedSize": 134217728,
      "sha384": "3a1f9c0d4e5b6a7c8d9e0f1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e"
    }
  }],
  "confexts": [{
    "id": "fleet-config",
    "image": {
      "path": "images/extensions/fleet-config.raw.zst",
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
separately proposes `scope: slotted` for Trident-managed sysext placement; it
handles `/etc` layers separately. Omitting `path` alone does not opt in.

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

Compute an effective set from COSI and operator-provided Host Configuration
entries, retaining each entry's source. Match old and new entries by
`(kind, id)`; replace when content changes, and reconcile placement or filename
changes. The extensions subsystem reads bundled members instead of downloading
them. `osconfig` enables the sysext/confext merge units for the effective set.
Validate SELinux compatibility against that set before provisioning; the
current check reads only Host Configuration entries.

[Disk streaming](../../Explanation/Disk-Streaming.md) must reject bundled
extensions: `osconfig` skips stream-image configuration, leaving the merge
units disabled.

#### Host Configuration Persistence

RFC 0838 owns inserting bundled extensions into the persisted Host
Configuration, whether `path` is explicit or Trident chooses placement. Use
`spec` and `spec_old` for new and previous sets instead of adding a separate
Host Status inventory. Keep generated entries distinct from operator-provided
ones so they do not collide with their own COSI source.

Host Configuration's `Extension` requires a URL and a SHA-384 of the raw DDI;
COSI `ImageFile.sha384` covers compressed bytes. Materialization must provide
a durable source reference and the raw hash, not copy the compressed hash.

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
for A/B). Stage temporary files on the destination filesystem for atomic
placement; the current fixed `/var/lib/extensions/.staging` falls back to a
non-atomic cross-filesystem copy.

#### Rollback

An A/B rollback restores the previous extension set only when placement is
slot-scoped. Writing a bundled image to a shared volume would change the running
slot and defeat rollback; see [Destination Validation](#destination-validation).

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

Still unsupported in enforcing mode on systemd 255. Check the effective set,
not only the Host Configuration lists, before Clean Install or A/B provisioning.

## Testing and Metrics

- v1.3 schema and valid/invalid samples, including missing or empty IDs;
  semantic path and duplicate-ID rules in Rust tests.
- Clean Install and A/B Update with both kinds, merge-unit activation and
  rollback (including absent `path` and slotted placement once supported).
- A COSI differing only in bundled extensions still selects A/B and restores
  the previous extension set on rollback.
- Reject mixed-source collisions, invalid destinations, enforcing SELinux,
  mismatched metadata/DDI IDs, and disk streaming.

## Implementation Plan

1. Publish COSI 1.3 schema and samples.
2. Read, validate and install bundled extensions; use RFC 0838 to persist them
   in Host Configuration.

## Counter-Arguments

### Drawbacks

Bundling still ties distribution to a new COSI; even extension-only changes
require an A/B update and reboot. It increases image size and gives hosts no
per-extension opt-out. Use Host Configuration for extensions that must move
independently. Older readers silently omit bundled extensions.

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

- **Operator override:** retain the hard error on bundled/Host Configuration
  collisions, or permit an explicit override when a hotfix is needed?

## Future Work

Avoiding an A/B update when only bundled extensions change would require
distinguishing OS content from extension content and preserving the previous
DDIs for runtime rollback. This RFC does neither.
