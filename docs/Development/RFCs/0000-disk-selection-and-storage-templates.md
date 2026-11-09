# 0000 Autonomous and Unattended Installs

- Date: 2026-11-09
- RFC PR: Not opened
- Issue: Not created

## Summary

Make normal Install autonomous: one Host Configuration can select suitable
disks and derive storage without interactive disk picking or hand-written
image layouts. Add per-disk `selector` objects and `storage.template` with
three types: `none` preserves authored storage, `image` reproduces COSI storage,
and `image-grow-last` also uses extra capacity for the last eligible filesystem.
Existing literal paths remain valid; templates do not switch to StreamDisk.

## Motivation and Goals

Today, [Disk.device](../../Reference/Host-Configuration/API-Reference/Disk.md)
requires a literal path. StreamDisk has a private smallest-fit selector and
derives configuration from the image. Its capacity threshold is COSI
`disk.size`: the original virtual disk size, **not compressed archive size**.[^selector]

Unattended provisioning needs reusable configurations across different disk
names and capacities. Disk strategies remove external path-replacement scripts;
image templates remove duplicated storage definitions. Once invoked with an
explicit policy, Install must finish without prompts or return a clear failure,
not fall back to interactive selection. This does not invent missing image
metadata or make existing-data erasure implicit.

## Scope

### Requirements

Preserve authored storage and boot-critical identities. Make selection
deterministic, safe, shared by all entry points, and immutable during servicing.

### Out of Scope

Unsupported: replacement/rebinding, shrink-to-fit, RAID/encryption inference,
arbitrary overrides, distribution-specific constants, and selectors with
adopted partitions. Adoption still requires an explicit path.

### Exit Criteria

An eligible host installs without prompts; unsafe targets fail before writes.
Restart, update, and rollback retain the original bindings.

## Dependencies

COSI 1.2 describes GPT, filesystems, and verity, but **not A/B pairing,
active-slot designation, inactive-slot mounts, or complete mount options**.
The translator leaves `storage.abUpdate` unset.[^derivation] Full A/B templates
need versioned deployment metadata. Until then, use authored storage for A/B;
never infer its relationships from labels.

## Public API Design

### New Fields and Values

These are proposed additions, not currently supported syntax:

| New field | Type / values | Default or constraint |
| --- | --- | --- |
| `storage.disks[].selector` | Selector object | Exactly one of `device` or `selector` per authored disk |
| `storage.template` | Template object | Omitted means `{type: none}` |
| `storage.template.type` | `none`, `image`, `image-grow-last` | `none` |
| `storage.template.disk` | Target object | Required for image types; forbidden for `none` |
| `storage.template.disk.device` | Device-path string | Exactly one of `device` or `selector` |
| `storage.template.disk.selector` | Selector object | Same selector schema as authored disks |

Both selector objects add exactly these fields:

| Field | Type / values | Default or constraint |
| --- | --- | --- |
| `strategy` | `first-nvme-fit`, `first-sd-fit`, `first-vd-fit`, `smallest-fit`, `largest-fit` | Required |
| `allowExistingData` | Boolean: `true`, `false` | `false`; `true` explicitly permits existing-data erasure |

No selector is implicit. Existing `storage.disks[].device` is unchanged.

| Template type | Storage behavior |
| --- | --- |
| `none` | Use existing authored storage; no image-derived layout |
| `image` | Derive storage from COSI; leave extra target capacity unused |
| `image-grow-last` | Derive storage from COSI and grow the final eligible partition/filesystem into extra capacity |

Image types require `image` and forbid authored disks, filesystems, verity, A/B,
RAID, encryption, or swap configuration. Conflicts are errors, not precedence
rules. Template targets cannot contain partition definitions.

### Examples and Strategy Semantics

Host Configuration fragments follow; existing filesystem definitions are
omitted in the first example.

```yaml
storage:
  template:
    type: none
  disks:
    - id: os
      selector:
        strategy: first-nvme-fit
      partitionTableType: gpt
      partitions:
        - id: esp
          type: esp
          size: 192MiB
        - id: root
          type: root
          size: grow
```

Fixed hardware can continue to use `device: /dev/disk/by-id/...` instead.

| Strategy | Selection among eligible, fitting disks |
| --- | --- |
| `first-nvme-fit` | First NVMe namespace, ordered by controller then namespace number |
| `first-sd-fit` | First whole `sd*` disk in natural letter order |
| `first-vd-fit` | First whole `vd*` virtio disk in natural letter order |
| `smallest-fit` | Least capacity |
| `largest-fit` | Greatest capacity |

For example, `nvme2n1` precedes `nvme10n1`, and `sdz` precedes `sdaa`.
Capacity ties use the same natural ordering, with family order NVMe, `sd`, `vd`.
This ordering describes the discovery snapshot, not identity across reboots.
These are Linux device families, not guarantees about physical attachment:
`sd*` includes SAS, SATA, and USB. Verify whole-disk/namespace identity and
transport through kernel topology; never use a prefix-only eligibility check.
The general strategies use these same three families.

Image templates supply their target instead of an authored disk list:

```yaml
image:
  url: https://example.com/os.cosi
  sha384: "<metadata-sha384>"
storage:
  template:
    type: image
    disk:
      selector:
        strategy: first-sd-fit
```

To consume extra target capacity and explicitly permit existing-data erasure:

```yaml
storage:
  template:
    type: image-grow-last
    disk:
      selector:
        strategy: first-vd-fit
        allowExistingData: true
```

This fragment uses the same `image` section.

## Implementation

### Resolution and Safety

Resolve metadata and inventory before provisioning. Authored-storage fit
includes fixed sizes, alignment/GPT overhead, and grow-partition minima,
including uncompressed payloads and filesystem/device-layer overhead.
Uncomputable minima fail. Image templates require at least COSI `disk.size`,
regardless of compressed/summed payload size; neither shrinks image geometry.

Automatic candidates must be writable whole disks with a recoverable stable
identity. Exclude USB/removable media, installation-media backing disks, live
root backing disks, mounted descendants, swap, and active device-mapper/RAID
holders. Trace backing-device relationships transitively and deduplicate
aliases; unsupported multipath identity fails. Explicit paths retain existing
adoption behavior, but never bypass protection against erasing live/media data.

By default, exclude disks with partition tables or recognized data signatures.
This is a conservative blank-disk check, not proof that no valuable bytes exist.
`allowExistingData: true` explicitly authorizes erasing the selected disk's
existing data; it cannot override the other exclusions.

Reserve explicit targets first, then resolve selectors in disk-list order,
removing each chosen identity from the candidate set. No backtracking or disk
reuse. No match, duplicate binding, ambiguous identity, or inventory failure
returns a structured error with required capacity and exclusion reasons.

Core Trident owns selection and safety for CLI and gRPC. Installer supplies
media identities/exclusions; its manual picker remains a separate interactive
workflow. Emit the layout, device identity, capacity, and erase scope before
writes without requiring confirmation in an explicitly unattended invocation.
Immediately before partitioning, revalidate identity, size, sector geometry,
and exclusions against that plan under a servicing lock. Hold the target device
open; disappearance or changed identity aborts, never selects another disk.

### Template Semantics

Require a supported COSI version, initially 1.2, with valid GPT regions and
matching logical sector size. Verify metadata and region checksums through the
existing image-integrity path. Derive partition order, offsets, types, flags,
labels, sizes, and UUIDs from GPT; derive filesystem identity, mounts, ESP, and
verity data/hash relationships only where metadata describes them.

Persist geometry and region-to-device mapping alongside the expanded Host
Configuration: the current translator alone cannot express every GPT detail.
Install must deploy mapped unmounted regions too, while retaining normal
Install boot/ESP orchestration.[^deployment] Layout replication is not a
byte-for-byte whole-disk copy; unallocated bytes are not image content.
Preserve filesystem and PARTUUID identities referenced by sealed UKIs or
verity; reject ambiguous references rather than regenerate them.

Missing required mount/slot metadata, conflicting ESPs, unsupported verity
representation, or unrepresentable boot constraints fail before erasure.
Never infer A/B from duplicate partition types, names, or apparent free slots.

`image-grow-last` chooses the highest-offset eligible partition: a writable
ext4 or Btrfs filesystem, outside ESP/boot, `/usr`, verity data/hash/signature
devices, and A/B pairs. It must also be the physically final partition.
Otherwise fail; do not move later partitions or silently fall back to `image`.
Grow only into contiguous trailing space, preserve its start/identity, align
the new end to sector and 4096-byte boundaries, and place the backup GPT at the
target's end. Verify the payload before filesystem growth and use a supported
filesystem-specific grow operation. Trident currently resizes writable ext
images; Btrfs growth requires additional support.[^deployment] Unsupported
growth fails in preflight, including when the final entry is a hash partition.

## Testing and Metrics

Acceptance covers unattended success/refusal, ordering/ties, capacity,
multi-disk reservation, exclusions, erase consent, unplug, and lost-journal
refusal. Verify template geometry/identities, unmounted regions, metadata
failures, GPT relocation, and safe growth. Report refusal reasons, not image
contents.

## Servicing

Persist bindings, expanded storage, geometry, template intent, and metadata
digest in a recoverable servicing journal **before destructive I/O**, then
retain them in installed Host Status. The provisioning environment must provide
durable journal storage outside the erase set; without it, refuse automatic
selector/template installation. Retry requires that journal, not renewed
discovery.

Updates and rollback reuse bindings and compare effective storage with the
persisted plan under existing storage-change invariants.[^servicing] A new COSI
may replace compatible payloads, not re-expand or resize installed storage.
Missing disks or journal state fail closed. Replacement/reselection requires
an explicitly new reprovisioning operation, never an ordinary retry.

Existing configurations stay valid. Older Trident rejects new fields; require
a supporting version.

## Counter-Arguments

Literal paths suit fixed hardware. Installer-only selection would duplicate
policy and omit CLI/gRPC. An object template carries its explicit target; a
scalar cannot. Strict metadata and journal requirements limit adoption but
prevent guesses and rebinding.

## Open Questions

Agree versioned A/B/mount metadata and recoverable installer journal
integration. Neither gap permits heuristic inference or transient-only
persistence.

[^selector]: [Current StreamDisk selector](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/stream.rs).
[^derivation]: [COSI specification](../../Reference/Composable-OS-Image.md) and [current translator](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/osimage/cosi/derived_hc.rs).
[^deployment]: [Current image deployment](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/engine/storage/image.rs).
[^servicing]: [Current storage validation](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/subsystems/storage/mod.rs).
