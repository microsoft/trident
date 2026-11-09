# 0000 Disk Selection and Storage Templates

- Date: 2026-11-09
- RFC PR: Not opened
- Issue: Not created

## Summary

Allow Host Configuration disks to select a target by strategy instead of device
path. Add an opt-in `storage.template` object that derives storage from a COSI,
optionally growing its last eligible filesystem. These are inputs to normal
Install, not a request to switch to StreamDisk.

## Motivation and Goals

Today, [Disk.device](../../Reference/Host-Configuration/API-Reference/Disk.md)
requires a literal path. StreamDisk has a private smallest-fit selector and
derives configuration from the image. Its capacity threshold is COSI
`disk.size`: the original virtual disk size, **not compressed archive size**.[^selector]

Users should be able to install onto the first suitable NVMe, SCSI-style, or
virtio disk without an external path-replacement script. Image-derived storage
should remove duplicated partition and filesystem definitions without inventing
information the image does not contain.

## Scope

### Requirements

Preserve explicit paths and authored storage. Make selection deterministic,
safe, shared by all entry points, and immutable during servicing. Replicate
image-described storage and preserve boot-critical identities.

### Out of Scope

Disk replacement, hotplug rebinding, automatic shrink-to-fit, RAID/encryption
inference, arbitrary template overrides, and distribution-specific partition
numbers or GUID constants. Selectors with adopted partitions are initially
unsupported; adoption continues to require an explicit path.

### Exit Criteria

The examples resolve to inspectable plans; unsafe targets fail before writes;
restart, update, and rollback retain the original bindings.

## Dependencies

COSI 1.2 provides GPT geometry and filesystem/verity metadata, but **not A/B
pairing, active-slot designation, inactive-slot mounts, or complete mount
options**. The current translator does not derive `storage.abUpdate`.[^derivation]
Full A/B template installs need a versioned COSI deployment-metadata contract.
COSI 1.2 templates alone do not enable A/B servicing. Use authored storage when
A/B is required; partition labels are not a substitute for that contract.

## Public API Design

The following are proposed Host Configuration fragments, not currently
supported syntax. Existing filesystem definitions are omitted in the first
example.

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

Each authored disk requires exactly one of existing `device` or new `selector`.
`selector.strategy` is required; `selector.allowExistingData` defaults to
`false`. No selector is implicit. Fixed hardware can continue to use
`device: /dev/disk/by-id/...` instead.

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

An image template supplies its own target because no disk list is authored:

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

This fragment uses the same `image` section. The template's `disk` also accepts
`device` instead of `selector`, but no partition definitions.

Omitting `template` equals `{type: none}`. `none` forbids a template disk.
`image` and `image-grow-last` require `image` and exactly one template target;
they forbid authored disks, filesystems, verity, A/B, RAID, encryption, or swap
configuration. Conflicts are validation errors, not precedence rules.

## Implementation

### Resolution and Safety

Resolve metadata and candidate inventory before provisioning. Authored-storage
fit means all fixed partitions, alignment/GPT overhead, and minimum space for
grow partitions, including uncompressed image payloads and applicable
filesystem/device-layer overhead. Fail if a safe minimum cannot be computed.
Image templates require at least COSI `disk.size`, even when the archive or
summed payload is smaller. Neither template shrinks image geometry.

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
media identities/exclusions and handles presentation or a manual picker. Show
the effective layout, device identity, capacity, and erase scope before writes.
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

Acceptance covers natural ordering/ties, misleading compressed sizes,
multi-disk reservation, media/holder exclusions, data-erasure opt-in, unplug
between planning and writes, and lost-journal refusal. Template coverage
includes GPT/UUID preservation, unmounted regions, missing A/B metadata,
larger-target GPT relocation, supported filesystem growth, and sealed-device
rejection. Report selected identity and refusal reasons without image contents.

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

Old configurations remain unchanged. Older Trident versions reject these new
fields; callers must require a supporting version.

## Counter-Arguments

Literal paths remain simpler for fixed hardware. Installer-only selection
would duplicate policy and omit direct CLI/gRPC users. A scalar template is
shorter but has nowhere to put its target; an object keeps that choice explicit.
Strict metadata and journal requirements limit immediate adoption but avoid
unsafe guesses and rebinding.

## Open Questions

Agree the COSI deployment-metadata extension for A/B roles and mount options,
and how installer environments expose recoverable journal storage. Neither
gap permits heuristic inference or transient-only persistence.

[^selector]: [Current StreamDisk selector](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/stream.rs).
[^derivation]: [COSI specification](../../Reference/Composable-OS-Image.md) and [current translator](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/osimage/cosi/derived_hc.rs).
[^deployment]: [Current image deployment](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/engine/storage/image.rs).
[^servicing]: [Current storage validation](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/subsystems/storage/mod.rs).
