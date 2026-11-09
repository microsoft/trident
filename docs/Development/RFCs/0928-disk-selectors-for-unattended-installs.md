# 0928 Disk Selection Strategies for Unattended Installs

- Date: 2026-11-09
- RFC PR: [microsoft/trident#928](https://github.com/microsoft/trident/pull/928)
- Issue: Not created

## Summary

Add an input-only `storage.disks[].selector` with a required selection strategy
as an alternative to an explicit device path. Resolve it to a concrete `device`
before saving the Host Configuration (HC). Storage remains authored, the
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

Each authored disk has exactly one of existing `device` or new `selector`.
The selector is an object with these input-only fields:

| Field under `storage.disks[].selector` | Type | Rule |
| --- | --- | --- |
| `strategy` | Enum below | Required; no default |
| `allowExistingData` | Boolean | Default `false`; explicit existing-data erasure opt-in |

| Strategy | Selection among eligible disks with sufficient capacity |
| --- | --- |
| `first-nvme-fit` | First NVMe namespace in natural controller/namespace order |
| `first-sd-fit` | First whole `sd*` disk in natural letter order |
| `first-vd-fit` | First whole `vd*` disk in natural letter order |
| `smallest-fit` | Least capacity across supported families |
| `largest` | Greatest capacity across supported families |

For example, `nvme2n1` precedes `nvme10n1`, and `sdz` precedes `sdaa`.
Capacity ties use the same natural ordering, with family order NVMe, `sd`, `vd`.
This is a discovery-snapshot order, not identity across reboots. All strategies,
including `largest`, require the authored storage to fit. A family-specific
strategy never falls back to another family.

Unknown fields/values, missing strategy, and specifying both `device` and
`selector` are validation errors. The strategies use whole disks/namespaces of
these three families, verified through device kind and topology, not name
prefixes alone. `sd*` can include SAS, SATA, and USB; it does not mean internal
SATA.

### Example

First fitting NVMe namespace, retaining the existing authored layout:

```yaml
storage:
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

Existing filesystem definitions are omitted. Fixed hardware can continue to
use `device: /dev/disk/by-id/...` instead of `selector`.

## Resolution and Safety

Compute each disk's minimum capacity from authored fixed sizes, alignment/GPT
overhead, and grow-partition minima, including uncompressed image payloads and
filesystem/device-layer overhead. Fail if a safe minimum cannot be computed.
Compressed archive size is not the capacity requirement.

Candidates must be writable whole disks with recoverable stable identities.
Exclude USB/removable media, installation-media and live-root backing disks,
mounted descendants, swap, and active device-mapper/RAID holders. Trace backing
relationships transitively and deduplicate aliases; ambiguous identity or
unsupported multipath fails.

By default, exclude disks with partition tables or recognized data signatures.
This is a conservative blank-disk check, not proof that no valuable bytes exist.
`allowExistingData: true` permits erasing the selected disk's existing data, but
never overrides live/media/read-only protections. Size and ordering alone do
not identify the intended OS disk. Explicit paths retain existing adoption
behavior without permission to erase live/media data.

Reserve explicit targets first, then resolve selectors in disk-list order,
removing each chosen identity from the candidate set. No backtracking or reuse.
No eligible disk, duplicate binding, or inventory failure produces a structured
error with required capacity and refusal reasons.

Replace each selector with its resolved `device` path, remove all selector
fields/options, and apply existing HC validation. Persist only the existing HC
representation, as if the user had authored the concrete paths directly.

Installer supplies media identities/exclusions and presents the resolved
identity, capacity, layout, and erase scope. Its manual picker remains a
separate workflow. Core revalidates the chosen identity, geometry, capacity,
and exclusions immediately before partitioning under a servicing lock.
Disappearance or changed identity aborts, never triggers another selection.

## Servicing

Checkpoint the resolved HC and necessary identity state before destructive I/O,
then retain the concrete HC in Host Status. Recovery requires durable state
outside the erase set; if unavailable, fail rather than rediscover. Do not save
strategies or selector options in the HC.

Retries, updates, and rollback retain the concrete binding under existing
storage-change invariants.[^servicing] Newly attached disks do not affect it.
Missing disks or state fail closed. Reselection requires explicit reprovisioning.
Existing configurations stay valid; older Trident rejects the new input fields.

## Testing and Metrics

Cover family selection, natural ordering/ties, capacity, multi-disk reservation,
media/holder exclusions, erasure opt-in, unplug, and restart without rebinding.
Verify the saved HC contains concrete paths and no selector fields. Report
selected identities and refusal reasons, not image contents.

## Counter-Arguments and Open Questions

Explicit stable paths remain preferable when capacity/order cannot distinguish
boot and data disks. Keeping this proposal strategy-only avoids a matching
language or a second policy input channel.

Agree how installer environments provide recoverable journal storage. This
does not permit transient-only bindings or automatic reselection.

[^stream]: [Current StreamDisk selector](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/stream.rs).
[^servicing]: [Current storage validation](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/subsystems/storage/mod.rs).
