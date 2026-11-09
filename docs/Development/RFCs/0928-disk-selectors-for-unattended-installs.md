# 0928 Disk Selectors for Unattended Installs

- Date: 2026-11-09
- RFC PR: [microsoft/trident#928](https://github.com/microsoft/trident/pull/928)
- Issue: Not created

## Summary

Add an input-only `storage.disks[].selector` as an alternative to an explicit
device path. A selector can identify a disk by matching properties, choose among
eligible disks with a strategy, or combine both. **A strategy is not required
when matching uniquely identifies a target.**
For caller-provided policy, `selector: external` explicitly delegates selection
outside the HC instead of allowing an unsolicited override.

Resolve selectors to concrete `device` paths before saving the Host
Configuration (HC). The persisted HC schema is unchanged, and servicing never
repeats selection. Storage layout remains authored; image-derived layouts are
a separate proposal.

## Motivation and Goals

One HC and bootable image should support unattended installation across servers
and VMs without per-host device-path substitution. Targets may appear as NVMe,
`sd*`, or `vd*`, alongside data disks and installation media. A stable identity
or boot-disk convention may be more useful than "the smallest disk."

Today, [Disk.device](../../Reference/Host-Configuration/API-Reference/Disk.md)
requires a literal path. StreamDisk has a private smallest-fit selector using
the image's original virtual-disk footprint, not compressed archive
size.[^stream] Normal Install needs its own supported selection input without
changing the authored storage graph.

Borrow [Netplan's matching model](https://netplan.readthedocs.io/en/stable/netplan-yaml/#properties-for-physical-device-types):
properties describe candidate devices and combine with AND. Unlike network
configuration that can apply to multiple interfaces, an installation disk
entry must bind to exactly one physical target.

## Scope

### Requirements

Support match-only, strategy-only, and combined selectors. Preserve explicit
paths, authored storage, and boot-critical identities. Keep policy in core
Trident, shared by CLI and gRPC. Unattended calls succeed or fail without
prompts.

### Out of Scope

Storage inference, A/B derivation, replacement/rebinding, shrink-to-fit,
arbitrary udev expressions, and selectors with adopted partitions. Adoption
continues to require an explicit path.

### Exit Criteria

Select one safe, fitting target per disk entry, expose its identity, and retain
the binding through restart, update, and rollback.

## Public API Design

### New Input-Only Fields

Each authored disk has exactly one of existing `device` or new `selector`.
`selector` is either an inline object below or the scalar `external`, meaning
"require policy from the caller." There is no parent configuration to inherit.

| Field under `storage.disks[].selector` | Type / values | Rule |
| --- | --- | --- |
| `match` | Non-empty object below | Optional if `strategy` is present |
| `match.name` | Kernel disk basename or glob | For example `vda`, `sd*`, `nvme*n*` |
| `match.serial` | Exact reported serial string | No globbing |
| `match.wwn` | Exact reported WWN string | No globbing |
| `strategy` | `first`, `smallest-fit`, `largest` | Optional if `match` is present |
| `allowExistingData` | Boolean | Default `false`; explicit existing-data erasure opt-in |

A valid inline selector contains a non-empty `match`, a `strategy`, or both. Empty
objects, unknown fields/values, and `allowExistingData` alone are invalid. No
selector or strategy is implicit.

All specified match properties must hold. Name matching uses case-sensitive
globs (`*`, `?`, character classes) against the whole-disk basename, not aliases
or partition names. Serial/WWN values match exactly; a missing attribute does
not match. Inventory retrieval failures abort rather than broaden the search.

Filter for eligibility and sufficient capacity, then apply matches. Without a
strategy, zero matches fail and multiple matches are an ambiguity error. With a
strategy, choose one from the matching set:

| Strategy | Selection |
| --- | --- |
| `first` | First in deterministic natural device-name order |
| `smallest-fit` | Least capacity that fits authored storage |
| `largest` | Greatest capacity among matching eligible disks |

Capacity ties use the same natural ordering. Initially support NVMe namespaces,
`sd*`, and `vd*` whole disks, ordered by family NVMe, `sd`, `vd`. Within NVMe,
order controller then namespace numbers; within `sd`/`vd`, use natural letter
order. Thus `nvme2n1` precedes `nvme10n1`, and `sdz` precedes `sdaa`. This is a
discovery-snapshot order, not identity across reboots. Verify device kind and
topology, never eligibility from a name prefix alone.

### Examples and Use Cases

First fitting NVMe namespace, retaining the existing authored layout:

```yaml
storage:
  disks:
    - id: os
      selector:
        match:
          name: "nvme*n*"
        strategy: first
      partitionTableType: gpt
      partitions:
        - id: esp
          type: esp
          size: 192MiB
        - id: root
          type: root
          size: grow
```

Existing filesystem definitions are omitted. Use `name: "sd*"` or `"vd*"` with
`first` for the other device families. `sd*` does not mean internal SATA; it can
include SAS and USB, so transport/media exclusions still apply.

Identify a provisioned virtual boot disk without a strategy:

```yaml
selector:
  match:
    serial: example-boot-disk
```

Other useful combinations:

| Selector | Use case |
| --- | --- |
| `match.name: vda` | Fixed VM device convention; reject if unavailable |
| `match.wwn: "<boot-disk-wwn>"` | Pin a physical disk despite kernel-name changes |
| `match.name: "nvme*n*"` plus `smallest-fit` | Prefer a smaller OS disk among NVMe devices |
| `strategy: smallest-fit` | Choose the smallest eligible disk across supported families |
| `strategy: largest` | Allocate a capacity-oriented single-disk host |

Family matches can still select a data disk. Matching, ordering, and capacity
are policy inputs, not proof that the intended OS disk was identified.

### Secondary Idea: Install RPC Selector

An HC can explicitly leave target policy to the caller while retaining its
authored layout. Fragment:

```yaml
storage:
  disks:
    - id: os
      selector: external
```

Optionally add `disk_selector` to `StageInstallRequest`, used by both `Install`
and `InstallStage`.[^installrpc] Proposed protobuf sketch:

```protobuf
message StageInstallRequest {
  trident.v1.HostConfiguration config = 1;
  DiskSelector disk_selector = 2;
}
```

The caller supplies the inline object form, including match-only selection.
For example, a request fragment using protobuf JSON field names:

```yaml
stage:
  diskSelector:
    match:
      name: "nvme*n*"
    strategy: smallest-fit
```

Before initial resolution, bind that policy to exactly one `external` entry;
other disk entries are unaffected. A missing policy, unsolicited policy when
no entry delegates, multiple external entries, or an empty/unresolved policy
is an error. The request policy cannot itself be `external`. It never overrides
a literal `device` or an inline selector.

The marker alone does not authorize erasure; the supplied policy retains the
same defaults, safety checks, and resolver. Without a supported caller-policy
channel, `external` fails rather than consulting the environment or inventing a
default. Resolve both marker and policy to a concrete device before saving HC;
neither is persisted. Reusable YAML can then accept different deployment
policies without editing. This remains secondary to the inline HC proposal.

## Implementation

Resolve metadata and inventory before provisioning. Fit includes all authored
fixed sizes, alignment/GPT overhead, and grow-partition minima, including
uncompressed image payloads and filesystem/device-layer overhead. Uncomputable
minima fail. Compressed archive size is never a capacity requirement.

Automatic candidates must be writable whole disks with recoverable stable
identities. Exclude USB/removable media, installation-media and live-root backing
disks, mounted descendants, swap, and active device-mapper/RAID holders. Trace
backing relationships transitively and deduplicate aliases; ambiguous identity
or unsupported multipath fails.

By default, exclude partition tables and recognized data signatures. This is a
conservative blank-disk check, not proof that no valuable bytes exist.
`allowExistingData: true` permits erasing the selected disk's existing data,
but never overrides live/media/read-only protections. Explicit paths retain
existing adoption behavior without permission to erase live/media data.

Reserve explicit targets first, then resolve selectors in disk-list order,
removing each chosen identity from the candidate set. No backtracking or reuse.
No match, ambiguity, duplicate binding, or inventory failure returns a
structured error with capacity, candidate identities, and refusal reasons.

Validate input syntax and explicit delegation first; replace each selector
with its resolved `device` path and remove all selector fields/options. Apply existing HC validation to
that concrete HC. Persistence and storage comparisons see only existing fields,
as if the user had authored the resolved paths.

Core owns selection and write-time safety. Installer supplies media
identities/exclusions; its manual picker is a separate workflow. Emit identity,
capacity, layout, and erase scope before writes. Revalidate identity, geometry,
capacity, and exclusions immediately before partitioning under a servicing
lock. Hold the target open; changes abort, never trigger another selection.

## Testing and Metrics

Cover match-only success/ambiguity, AND semantics, absent attributes, globs,
ordering/ties, capacity, multi-disk reservation, exclusions, erase consent,
unplug, and lost-journal refusal. Cover external-policy success, missing or
unsolicited policies, ambiguous delegation, and rejection of recursive
deferral. Verify the saved HC has concrete paths and no selector/marker fields.
Report identities and refusal reasons, not image contents.

## Servicing

Checkpoint the resolved HC and necessary identity state before destructive I/O,
then retain the concrete HC in Host Status. Recovery requires durable state
outside the erase set; if unavailable, fail rather than rediscover. Do not save
match rules, strategies, or selector options in the HC.

Retries, updates, and rollback retain the concrete binding under existing
storage-change invariants.[^servicing] They never re-evaluate caller policy;
newly attached disks do not affect the target.
Missing disks or state fail closed. Reselection requires explicit
reprovisioning. Existing configurations stay valid; older Trident rejects new
input fields.

## Counter-Arguments

Literal stable paths remain simpler for fixed hardware. Mandatory strategies
would force an arbitrary ordering policy onto uniquely identified disks.
Installer-only matching would duplicate policy and omit direct CLI/gRPC users.
The match object adds structure but separates identity constraints from choice.

## Further Selector Ideas

These are candidates for discussion, **not additional fields in this proposal**:

| Possible match | Use case / caution |
| --- | --- |
| Stable topology or by-path glob | A boot bay/controller shared across hosts; identifies a slot, not a physical disk |
| Model/vendor | A fleet's dedicated boot-device class; identical data disks remain ambiguous |
| Transport or rotational state | Constrain NVMe/virtio or SSD versus HDD; neither proves boot-disk intent |
| Minimum/maximum capacity | Exclude large data disks or match a boot-device size class; also enforce authored-layout fit |

Prefer a small set of typed attributes over arbitrary udev queries or a general
expression language. Explicit exclusions may be useful later; core safety
exclusions must remain non-overridable.

## Open Questions

Agree serial/WWN inventory normalization, the first useful additional match
attributes, and recoverable journal integration. These do not justify a default
strategy, ambiguous match-only installation, or rebinding during servicing.

[^stream]: [Current StreamDisk selector](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/stream.rs).
[^servicing]: [Current storage validation](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/crates/trident/src/subsystems/storage/mod.rs).
[^installrpc]: [Current Install request](https://github.com/microsoft/trident/blob/bfeff0fd37193b011fe54fa23a766b901d3cf0f8/proto/trident/v1preview/install_service.proto).
