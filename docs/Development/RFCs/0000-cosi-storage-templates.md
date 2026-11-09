# 0000 COSI Storage Templates

- Date: 2026-11-09
- RFC PR: Not opened
- Issue: Not created
- Status: Design draft; A/B derivation remains unresolved

## Summary

Propose an input-only `storage.template` that expands a COSI's storage
description into an ordinary Host Configuration (HC). `none` preserves authored
storage, `image` reproduces the image layout, and `image-grow-last` additionally
uses extra target capacity for the final eligible mutable filesystem.

This is independent of disk selection: an explicit device path is sufficient.
The [disk-selector proposal](https://github.com/microsoft/trident/pull/928) may
eventually provide another way to resolve that path, but is not a prerequisite.
Templates remain normal Install, not an implicit switch to StreamDisk.

## Motivation and Goals

Image-following installers should not duplicate every partition, filesystem,
verity relationship, and boot identity in another configuration. That copy can
drift when the image changes. Opt-in templates make the image authoritative
where it contains sufficient information; authored storage remains available
for customized layouts.

However, reproducing partition bytes is not equivalent to deriving servicing
semantics. A COSI can contain A/B partitions without describing their pairing.
That gap is why this design is separate from target selection.

## Scope

### Requirements

Preserve image-described geometry and boot-critical identities. Expand templates
to existing HC values before validation/persistence; never save the template or
its options in the HC. Report unsupported inference before destructive writes.

### Out of Scope

Automatic disk selection, shrink-to-fit, distribution-specific GUIDs or
partition numbers, RAID/encryption inference, and arbitrary layout overrides.

### Exit Criteria

A supported image produces an inspectable, concrete Install plan. Unsupported
metadata fails clearly. A/B derivation requires an agreed metadata/mapping
contract before this RFC can promise A/B-capable template installs.

## Dependencies and Unresolved A/B Semantics

COSI 1.2 describes GPT geometry, filesystem images, mounts, and verity hash
images. It does **not** define A/B volume pairs, active-slot designation,
inactive-slot mounts, or complete mount options. The current translator leaves
`storage.abUpdate` unset.[^derivation]

Do not infer pairs from labels, suffixes, duplicate types, or partition numbers.
GPT alone cannot establish whether the author intends A/B servicing. Until the
contract is settled, use authored storage when A/B is required.

Two approaches need further evaluation: versioned deployment metadata inside
COSI, or explicit UUID-based pairing/mount inputs alongside the template. The
latter would be a narrow semantic override, not a second hand-authored layout.
Neither approach is specified by this draft. The API below is provisional,
particularly how callers declare required servicing semantics.

## Public API Design

All additions are **input-only**:

| Field | Type / values | Default or constraint |
| --- | --- | --- |
| `storage.template` | Object | Omitted means `{type: none}` |
| `storage.template.type` | `none`, `image`, `image-grow-last` | `none` |
| `storage.template.disk` | Target object | Required for image types; forbidden for `none` |
| `storage.template.disk.device` | Explicit device-path string | Required target in this standalone proposal |

```yaml
image:
  url: https://example.com/os.cosi
  sha384: "<metadata-sha384>"
storage:
  template:
    type: image
    disk:
      device: /dev/disk/by-id/example-os-disk
```

Changing `type` to `image-grow-last` requests safe final-filesystem growth.
`none` retains current authored storage. Image types forbid authored disks,
filesystems, verity, A/B, RAID, encryption, or swap configuration; conflicts are
errors, not precedence rules. A future semantic-override design would need to
explicitly revise this rule.

Remove `storage.template` after expansion, including `type: none`. The saved HC
contains ordinary device paths, partitions, filesystems, and related storage
values. Growth becomes a concrete partition size. Its persisted schema is
unchanged.

## Implementation

### Image-Derived Storage

Initially require supported COSI 1.2 GPT regions and matching logical sector
size. Verify metadata and payloads through existing image-integrity mechanisms.
The target must fit COSI `disk.size`, the original virtual-disk footprint, not
the compressed archive size or summed payload sizes.[^cosi]

Derive partition order, offsets, types, flags, labels, sizes, and UUIDs from GPT.
Use filesystem/boot metadata for identity, mount points, ESP, and verity
data/hash relationships only where authoritative. Required geometry and
region-to-device mappings may need internal servicing state beyond what the
current HC translator expresses; they are not new HC fields.

Normal Install must also handle mapped unmounted regions and preserve its
normal boot/ESP orchestration. Current filesystem-oriented deployment alone
does not cover all raw image regions.[^deployment] Layout replication is not a
byte-for-byte whole-disk copy; unallocated bytes are not image content.

Preserve filesystem UUIDs and PARTUUIDs referenced by sealed UKIs or verity.
Reject ambiguous references, conflicting ESPs, unsupported verity
representations, and unrepresentable boot constraints. Do not silently drop
required servicing semantics merely because layout bytes can be copied.

### Final-Filesystem Growth

Choose the highest-offset eligible partition: a writable ext4 or Btrfs
filesystem outside ESP/boot, `/usr`, verity data/hash/signature devices, and A/B
pairs. It must also be the physically final partition. Otherwise fail; do not
move later partitions or silently fall back to `image`.

Grow only into contiguous trailing space. Preserve the start and identity,
align the new end to sector and 4096-byte boundaries, and place the backup GPT
at the target's end. Verify the payload before filesystem growth and use a
supported filesystem-specific operation. The current image-resize path is
ext-only; Btrfs requires additional support.[^deployment] Unsupported growth
must fail in preflight.

## Testing and Metrics

Cover layout/identity preservation, missing or ambiguous A/B metadata, unmounted
regions, sector mismatch, larger-target GPT relocation, safe growth, and sealed
device rejection. Verify that saved HC contains no template fields. Expose the
resolved layout and refusal reasons without image contents.

## Servicing

Apply existing target safety checks; a template is not an erasure-policy
override. Protect live/media backing devices and mounted/read-only targets.
Bind the resolved storage and any required internal region mappings during
initial Install and checkpoint before destructive writes.

Retries, updates, and rollback consume resolved state, not the template. A new
COSI may replace compatible payloads, not re-expand or resize installed storage.
Existing storage-change invariants continue to apply.[^servicing] Lost state or
missing bindings fail closed; reprovisioning must be explicit.

## Counter-Arguments

Authored storage is more explicit and already expresses servicing relationships.
Raw streaming can reproduce an image without solving all HC derivation
questions, but does not replace the normal Install contract.

## Open Questions

Resolve the A/B metadata versus explicit-mapping choice, including inactive
data/hash relationships, active slot, and mounts. Determine which boot/GPT
details need additional internal state and how that state survives recovery.
Agree growth eligibility when metadata does not establish writability.

[^derivation]: [COSI specification](../../Reference/Composable-OS-Image.md) and [current translator](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/osimage/cosi/derived_hc.rs).
[^cosi]: [Current COSI disk metadata](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/osimage/cosi/metadata.rs).
[^deployment]: [Current image deployment](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/engine/storage/image.rs).
[^servicing]: [Current storage validation](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/subsystems/storage/mod.rs).
