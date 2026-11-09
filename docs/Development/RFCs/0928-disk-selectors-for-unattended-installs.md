# 0928 Disk Selection Strategies for Unattended Installs

- Date: 2026-11-09
- RFC PR: [microsoft/trident#928](https://github.com/microsoft/trident/pull/928)
- Issue: Not created

## Summary

Add an input-only `storage.disks[].selector` with a required selection strategy
as an alternative to an explicit device path. Resolve it to a stable,
identity-based `device` reference before saving the Host Configuration (HC).
Storage remains authored, the
persisted HC schema is unchanged, and servicing never repeats selection.

## Motivation and Goals

One HC and bootable image should support unattended installation across servers
and VMs without per-host device-path substitution. Targets may appear as NVMe,
`sd*`, or `vd*`, alongside data disks and installation media.

Today, [Disk.device](../../Reference/Host-Configuration/API-Reference/Disk.md)
requires a literal path. StreamDisk has a private smallest-fit selector using
the image's original virtual-disk footprint, not compressed archive
size.[^stream] Normal Install needs a supported selection input without
changing the authored storage graph.

## Scope

Support a small strategy enum, preserve explicit paths, and keep selection and
safety in core Trident for all existing entry points. Unattended calls succeed
or fail without prompts.

Property matching, caller-supplied policy/RPC extensions, storage inference, A/B
derivation, replacement/rebinding, and shrink-to-fit are out of scope. Selectors
with adopted partitions are initially unsupported; use explicit paths for
adoption. Storage templates remain a separate proposal.

## Public API Design

### Selector fields

Each authored disk has exactly one of existing `device` or new `selector`.
The selector is an object with these input-only fields:

| Field under `storage.disks[].selector` | Type | Rule |
| --- | --- | --- |
| `strategy` | Enum below | Required; no default |
| `allowExistingData` | Boolean | Default `false`; explicit existing-data erasure opt-in |

### Available strategies

| Strategy | Selection among eligible disks with sufficient capacity |
| --- | --- |
| `first-nvme-fit` | First NVMe namespace in natural controller/namespace order |
| `first-sd-fit` | First whole `sd*` disk in natural letter order |
| `first-vd-fit` | First whole `vd*` disk in natural letter order |
| `smallest-fit` | Least capacity across supported families |
| `largest` | Greatest capacity across supported families |
| `tiered-smallest-fit` | First eligible family in the priority order below, then least capacity within that family |
| `tiered-largest` | First eligible family in that priority order, then greatest capacity within that family |

### Tier preference order

Both tiered strategies use fixed priority:

1. NVMe
2. `sd`
3. `vd`
4. `mmcblk`
5. `hd`

Skip a family if it has no eligible disk large enough. Family preference
outranks size; select the smallest or largest fitting disk within the first
eligible family. Global `smallest-fit` and `largest` rank by capacity instead.

### Worked example: 32 GiB requirement

For a 32 GiB requirement, consider NVMe disks of 128/256 GiB and `sd` disks
of 64/1024 GiB. Global `smallest-fit` chooses the 64 GiB `sd` disk; `largest`
chooses the 1024 GiB `sd` disk. The tiered variants choose the 128 GiB and
256 GiB NVMe disks, respectively.

### Edge-case decisions

- **Tie-breaking:** Capacity ties use that family order, then natural
  device-name order. Compare NVMe controller/namespace numbers and `mmcblk`
  indices numerically; use natural letter order for `sd`, `vd`, and `hd`.
  For example, `nvme2n1` precedes `nvme10n1`, `mmcblk2` precedes `mmcblk10`,
  and `sdz` precedes `sdaa`.
- **Identity and capacity:** This is a discovery-snapshot order, not identity
  across reboots. All strategies require the authored storage to fit. The three
  family-specific strategies never fall back to another family.
- **Validation:** Unknown fields/values, missing strategy, and specifying both
  `device` and `selector` are validation errors.
- **Supported devices:** Supported families are NVMe namespaces, `sd*`, `vd*`
  (virtio), `mmcblkN` (non-removable eMMC user-area disks), and `hd*` (legacy
  IDE names). Verify whole-device kind and topology, not prefixes alone.
- **Family versus transport:** `sd*` can include SAS, SATA, USB, and VM disks.
  VMs can also expose NVMe, so this is a device-family preference, not a
  physical-versus-virtual guarantee.

### Configuration example

Smallest fitting disk in the first eligible tier, retaining the authored layout:

```yaml
storage:
  disks:
    - id: os
      selector:
        strategy: tiered-smallest-fit
      partitionTableType: gpt
      partitions:
        - id: esp
          type: esp
          size: 192MiB
        - id: root
          type: root
          size: grow
```

Existing filesystem definitions are omitted. Fixed hardware can continue to
use `device: /dev/disk/by-id/...` instead of `selector`.

## Resolution and Safety

### Capacity requirement

Compute each disk's minimum capacity from authored fixed sizes, alignment/GPT
overhead, and grow-partition minima, including uncompressed image payloads and
filesystem/device-layer overhead. Fail if a safe minimum cannot be computed.
Compressed archive size is not the capacity requirement.

### Candidate eligibility

- **Device requirements:** Candidates must be writable whole disks with
  recoverable stable identities.
- **USB and removable media:** Exclude USB-backed devices even when reported
  as non-removable. Exclude all removable media regardless of transport.
  These are hard exclusions for every strategy; erasure opt-in cannot bypass
  them.
- **Other protected devices:** Exclude installation-media and live-root backing
  disks, mounted descendants, swap, and active device-mapper/RAID holders.
- **Identity:** Trace backing relationships transitively and deduplicate
  aliases; ambiguous identity or unsupported multipath fails.
- **eMMC:** For `mmcblk`, accept only the whole non-removable eMMC user-area
  device. Exclude SD cards, partitions, boot0/boot1, and RPMB regions. Boot
  regions remain excluded even if their default write protection has been
  disabled.[^mmc]

### Existing data and erasure

- **Default:** By default, exclude disks with partition tables or recognized
  data signatures. This is a conservative blank-disk check, not proof that no
  valuable bytes exist.
- **Opt-in:** `allowExistingData: true` permits erasing the selected disk's
  existing data, but never overrides live/media/read-only protections.
- **Intent:** Size and ordering alone do not identify the intended OS disk.
  Explicit paths retain existing adoption behavior without permission to erase
  live/media data.

### Allocation and resolution

1. Reserve explicit targets first.
2. Then resolve selectors in disk-list order, removing each chosen identity
   from the candidate set.

No backtracking or reuse.
No eligible disk, duplicate binding, or inventory failure produces a structured
error with required capacity and refusal reasons.

Replace each selector with an unambiguous identity-based `device` reference,
such as `/dev/disk/by-id/wwn-...`, then remove all selector fields/options and
apply existing HC validation. If no stable reference is available, fail
resolution. Persist that reference, not its current kernel enumeration path
such as `/dev/sda` or `/dev/nvme0n1`. The HC representation is unchanged, as if
the user had authored the stable reference directly.

### Final validation

Installer supplies media identities/exclusions and presents the resolved
identity, capacity, layout, and erase scope. Its manual picker remains a
separate workflow. Core revalidates the chosen identity, geometry, capacity,
and exclusions immediately before partitioning under a servicing lock.
Disappearance or changed identity aborts, never triggers another selection.

## Servicing

Selectors are input-only. Save only the resolved HC and stable disk bindings,
never selectors, strategies, or selector options.

Before any destructive install work, durably checkpoint the resolved bindings.
Current clean-install staging clears the saved HC before partitioning and only
saves the new HC after staging succeeds.[^staging] The implementation must keep
the checkpoint through that reset and any staging failure.

The checkpoint must survive a live-image restart, outside RAM and the disks
being rewritten. If that durability cannot be provided, stop before disk
writes. A retry or restart must recover the checkpoint and reuse its disks,
even if they are now partially partitioned. If recovery fails or a bound disk
is missing, stop rather than selecting another disk.

After installation, updates and rollback use the saved stable references just
like authored `device` paths, under existing storage validation.[^servicing]
They do not run selection again. Existing configurations continue to work;
older Trident versions reject the new input fields.

## Implementation

The initial implementation should support:

- `tiered-smallest-fit`
- `smallest-fit`

All other strategies are optional future additions, not requirements for initial
delivery. Reject strategies that are not implemented rather than substituting
another strategy. Shared safety, resolution, and persistence rules still apply.
Initial delivery includes stable-reference resolution and crash-safe binding
persistence, not just the selection algorithms.

## Testing and Metrics

Test each strategy as it is implemented. Cover global versus tiered selection,
skipping unsafe/undersized
families, natural ordering/ties, capacity, multi-disk reservation, media/holder
exclusions (including USB devices reported as non-removable and non-USB
removable media), writable eMMC boot-region rejection, erasure opt-in, unplug, and
restart without rebinding. Interrupt after partitioning, restart the live image
with another blank disk attached, and verify recovery uses the same checkpointed
disk rather than running selection again. Also cover unavailable checkpoint
storage and kernel device renumbering across restart.
Verify the saved HC contains stable identity-based paths and no selector fields.
Report selected identities and refusal reasons, not image contents.

## Counter-Arguments

Explicit stable paths remain preferable when capacity/order cannot distinguish
boot and data disks. Keeping this proposal strategy-only avoids a matching
language or a second policy input channel.

[^stream]: [Current StreamDisk selector](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/stream.rs).
[^servicing]: [Current storage validation](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/subsystems/storage/mod.rs).
[^staging]: [Current clean-install staging and Host Status reset](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/engine/clean_install.rs).
[^mmc]: [Linux MMC device partitions and boot-region protection](https://docs.kernel.org/driver-api/mmc/mmc-dev-parts.html).
