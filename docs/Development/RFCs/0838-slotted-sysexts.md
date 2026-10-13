# 0838 Slotted Extension Storage

- Date: 2026-10-13
- RFC PR: [microsoft/trident#838](https://github.com/microsoft/trident/pull/838)
- Issue: TBD

## Summary

Trident requires every extension destination to sit on an A/B volume when A/B is
configured. That is one way to make an extension follow the slot, and it costs a
partition pair per extension directory.

This adds a second. An extension marked `scope: slotted` is stored per volume
under a Trident-owned store. A small component resolves the booted slot early in
boot and exposes that slot's content before anything consumes it. Slot-scoping
becomes a property of the extension rather than of the partition table.

The same component serves `/etc` layers, which otherwise have no viable
mechanism on systemd 255.

## Motivation and Goals

`validate_extension_images_locations` (`config/host/mod.rs:218`) rejects any
extension directory that does not resolve to a device with A/B capabilities.
The reasoning is sound: extensions on shared storage change the running slot
immediately and do not roll back.

The cost is that the only way to satisfy it is a dedicated A/B volume pair.
Trident's own reference configuration
(`tests/e2e_tests/trident_configurations/root-verity/trident-config.yaml`)
spends four partitions and 2.4 GB on it. That price buys only slot-scoping, and
it is charged in the currency hardest to change:

- **Partitions are fixed at install.** The size is a guess made before anyone
  knows what the host will carry; growing it means repartitioning.
- **Some layouts cannot pay it.** Where the partition table is published, signed
  or attested, adding a pair is not a configuration change. A layout whose only
  A/B volume is a verity-sealed `/usr` has none to offer on any terms.
- **It is all-or-nothing per directory.** An extension that should follow the
  slot and one that should persist cannot share a location.

The check conflates *being slot-scoped*, the property that matters, with *being
on an A/B block device*, one implementation of it.

### Why content matching alone is not sufficient

The obvious alternative is to place both slots' copies in one search directory
and let `systemd-sysext` merge whichever matches the booted OS. Version
mismatches are skipped quietly, so this very nearly works, and on a suitable
platform it needs no new code at all.

It fails for two independent reasons, either sufficient on its own:

1. **Deploying one image under two filenames requires systemd v256.**
   `path_extract_image_name()` truncates an image name at the first `_` or `+`
   (`src/basic/os-util.c`), letting `platform_1.raw` and `platform_2.raw` share
   one inner `extension-release.platform`. That function does not exist before
   v256; it arrived in commit `37543971` (2024-03-28). Earlier systemd requires
   each copy to carry its own `extension-release`, so a signed image must be
   repacked per slot.
2. **The OS version must actually differ between slots.** Matching uses `ID=`,
   then `SYSEXT_LEVEL=` when defined on both sides, otherwise `VERSION_ID=`. A
   platform whose `VERSION_ID` is coarser than its build cadence, or which can
   deploy the same image to both slots, puts identical values in both and both
   copies match.

Both conditions hold on the first intended consumer. Content matching remains
the better mechanism where a platform satisfies both, and this design does not
preclude adopting it there. See [Future possibilities](#future-possibilities).

### Goals

Slot-scoped extensions without a dedicated volume pair; rollback restores the
previous set with no servicing operation; a workable `/etc` story on systemd
255; no new partitions, no sealed-`/usr` change, no modification to systemd.

### Non-goals

Changing `systemd-sysext` matching semantics. Removing the A/B volume route,
which stays better where a pair is affordable. Extensions on the ESP.

## Scope

Sysexts, confexts and `/etc` layers declared in the Host Configuration, on clean
install and A/B update. Runtime update does not change slot and is unaffected.

## Design

### Selection is recomputed every boot, never persisted

When firmware falls back to the previous slot on its own, no Trident code runs.
Anything written during servicing would be stale with nothing to correct it, so
the selection must be derived from what actually booted.

Verified: Trident's A/B rollback path invokes no subsystems.
`rollback::validate_boot` only reconciles Host Status, and
`manual_rollback::stage_rollback` touches only the pcrlock policy. Only the
runtime path passes `&mut subsystems`.

### Both halves run in the initrd

The `/etc` half has no choice. On a platform that assembles `/etc` as an overlay
whose lowerdir lives inside `/usr`, anything contributing to `/etc` must act
before that overlay is mounted, which happens in the initrd.

The sysext half can then ride along, because `/run` is bind-moved across
switch-root (`src/shared/switch-root.c`: `{ "/run", MS_BIND, MS_BIND|MS_REC }`).
Symlinks written to `/run/extensions` in the initrd are still present when
`systemd-sysext` merges later in the real root. Their targets must be written as
post-switch-root paths, so they are dangling in the initrd and correct
afterwards.

Unifying is not merely possible, it is better. The slot is resolved **once**, so
the sysext set and the `/etc` layer cannot disagree about which slot they came
from. It also removes a large amount of accidental complexity that a real-root
systemd unit would carry:

| Problem with a real-root unit | In the initrd |
| --- | --- |
| `WantedBy=` creates a `Wants=`, which `systemd.unit(5)` states cannot fail the transaction, so the unit does not fail closed | No unit; failing in dracut is native |
| Ordering anchor is subtle: `ensure-sysext.service` carries `BindsTo=` and `After=systemd-sysext.service`, so it runs *after* the merge and is the wrong edge | Nothing to order against |
| A platform-specific `ConditionKernelCommandLine=` would block every other mode from running | No unit condition |
| Unit enablement needs a preset or scriptlet | No unit to enable |
| Store must be available at `local-fs.target` | `/sysroot` is mounted by definition |

A real-root unit remains the fallback for platforms that cannot modify their
initrd. The mode selector therefore chooses *where* the loader runs as well as
how it detects the slot.

### Why `/etc` payloads are confext images

`systemd-confext` is the natural mechanism and is unusable here: on systemd 255
it mounts `/etc` read-only. Mutable overlays arrived in v256; Flatcar's backport
is 1310 lines across five files, and modifying systemd is out of scope.

The resolution is to keep the **format** and replace the **mount**. Payloads are
confext-format DDIs, carrying
`/etc/extension-release.d/extension-release.<name>` (`src/basic/os-util.c`).
The loader mounts them read-only and adds them as additional lowerdirs to the
existing `/etc` overlay, whose upperdir is unchanged, so `/etc` stays mutable.

This is preferred over tarballs or plain directory trees because:

- **It is forward-compatible.** The payload is already a valid confext. When a
  platform reaches systemd 256 or later, the identical artefact is handed to
  native `systemd-confext --mutable=yes` and the custom mounting is deleted. The
  custom mounting is a bridge, not a fork. Azure Linux 4.0 ships systemd 258.4,
  so this transition is concrete rather than hypothetical.

  Two things do not change at that transition, and both matter. The loader
  itself remains, because systemd has no concept of A/B slots: *selection* stays
  the platform's job, and only *mounting* becomes native. And native mounting
  reintroduces `extension-release` matching, which the custom lowerdir mount
  bypasses entirely.
- **It reuses everything.** Same build and signing pipeline as sysexts, same
  Trident download and hash-verification path, verity available by DDI
  convention.
- **It is atomic.** A loop mount either appears or does not. A tarball
  extraction can half-fail, and would also pay extraction cost on every boot.
- **SELinux labels travel in the filesystem image**, rather than in tar metadata.

#### Payloads must declare `ID=_any`

Because slot selection is the platform's job and not systemd's, the payload must
not additionally be subject to OS-version matching. A payload declaring a
concrete `ID=` would merge under the custom mount, which ignores matching, and
then silently stop merging the day a platform switches to native confext
mounting. That is the failure mode this RFC exists to prevent, reappearing at
the point of transition.

Declaring `ID=_any` bypasses matching on every systemd version. Selection
remains entirely with the loader, which is what slot-scoping requires anyway,
and the move to native mounting changes no artefact. Deciding this late would
mean rebuilding every payload at transition time, so it is specified here.

Adding lowerdirs does not increase filesystem stacking depth, because depth is
the maximum of the layers plus one and every lowerdir sits on a plain
filesystem. This matters: *reordering* the sysext merge ahead of the `/etc`
overlay would instead place `/etc` on top of the `/usr` overlay, reaching depth
2, and any confext merged afterwards would reach 3 and fail.
`FILESYSTEM_MAX_STACK_DEPTH` is 2 (`include/linux/fs.h`), enforced in
`fs/overlayfs/super.c`.

### Selection is a set, not a sequence

Creating N symlinks exposes a prefix of the intended set if it fails partway,
which matters where a driver and its userspace libraries must move together.
The loader prepares the complete set and commits it with a single rename.

## Public API Design

### Today

```yaml
os:
  sysexts:
    - url: https://example.com/tooling.raw   # http(s) | file | oci
      sha384: <hash>
      path: /var/lib/extensions/tooling.raw  # optional; allow-listed dir, *.raw
```

`path` is the destination, defaulting to `/var/lib/extensions/{name}.raw`. When
`storage.abUpdate` is configured it must also resolve to an A/B volume.

### Proposed

```yaml
os:
  # Root of the per-volume extension store.
  # Default /var/trident/extensions. Must be on storage an A/B update does not
  # replace, and must not be a systemd-sysext search path.
  extensionStore: /mnt/data/trident/extensions

  sysexts:
    # unchanged, and still the default
    - url: https://example.com/tooling.raw
      sha384: <hash>

    # new
    - url: https://example.com/platform.raw
      sha384: <hash>
      scope: slotted

  # new: confext-format images contributed as /etc overlay layers
  etcLayers:
    - url: https://example.com/platform-etc.raw
      sha384: <hash>
      scope: slotted
```

| `scope` | Where the bytes live | Where it is activated |
| --- | --- | --- |
| `shared` (default) | At `path`. | At `path`. Today's behaviour. |
| `slotted` | `<extensionStore>/{a,b}/` | `/run/extensions`, or as an `/etc` lowerdir |

A slotted extension always surfaces in a loader-owned location, so `path` does
not apply to it and is rejected.

`etcLayers` is a separate list rather than a `scope` on `os.confexts`, because
the two are activated by different mechanisms and only one of them is
`systemd-confext`. If a platform later gains mutable confext support, entries
can migrate to `os.confexts` unchanged.

### The validation change

`validate_extension_images_locations` is relaxed, not removed. Its requirement
becomes *the destination is slot-scoped*, satisfied either way:

| Configuration | Rule |
| --- | --- |
| No `abUpdate` | No check, as today. |
| `abUpdate`, `scope: shared` | `path` must be on an A/B volume. Unchanged. |
| `abUpdate`, `scope: slotted` | `path` need not be; the store plus the loader provide the scoping. |

This is only safe where the platform actually provides a loader, so validation
must test a **platform capability**, not merely that A/B is configured.

### Validation

| Condition | Result |
| --- | --- |
| `scope` absent | `shared`. Identical to today. |
| `scope: slotted` with `path` | Reject. The destination is loader-owned. |
| `scope: slotted`, no `abUpdate` | Reject, naming the absent configuration. |
| `scope: slotted`, no loader capability | Reject. See [Rollout](#rollout). |
| `extensionStore` inside a search path | Reject: both slots' copies would be discovered. |
| `extensionStore` on an A/B volume | Reject: it must outlive the slot it describes. |
| Two slotted entries resolving to the same basename | Reject: they would collide. |
| `etcLayers` entry without confext metadata | Reject at deploy time, after mount. |

### Backward compatibility

All new fields default, so existing Host Configurations parse and behave
identically. The relaxed check is strictly more permissive, so anything valid
today stays valid. The schema gains three optional properties.

## Implementation

```
<extensionStore>/
├── a/
│   ├── manifest.json
│   ├── sysext/platform.raw
│   └── etc/platform-etc.raw
└── b/ ...

/run/extensions/platform.raw -> <extensionStore>/a/sysext/platform.raw
/etc  overlay lowerdir list  += <mounted platform-etc.raw>
```

The store is outside every search path, so its contents are invisible to
`systemd-sysext` until selected. Both slot directories persist across an update,
so rollback needs no re-download.

### Servicing writes the store, atomically

The extensions subsystem already runs on clean install and A/B update via
`provision()`, documented as "migrate state from A-partition to B-partition (or
vice versa)". It resolves the target volume with
`EngineContext::get_ab_update_volume()`, already used to select the per-slot
verity addon, and writes into `<store>/<target>/`.

Placement must be atomic. A malformed `.raw` is not skipped like a version
mismatch: `image_read_metadata` errors propagate in
`image_discover_and_read_metadata` (`src/sysext/sysext.c`) before any per-image
tolerance applies, so **one bad image blocks every extension on the host**.
Directory-type images are tolerated; `.raw` are not.

Trident today falls back to a non-atomic `fs::copy` when `rename` fails across a
filesystem boundary (`crates/trident/src/subsystems/extensions/mod.rs:525-540`).
This is a prerequisite fix, not a consequence of this design.

### The store carries a manifest

Globbing `*.raw` cannot distinguish "no extensions configured" from "storage
lost", "mount failed" or "update interrupted", and treating all of those as
success produces a quietly extension-less host.

Each slot directory carries a manifest naming the expected images and hashes. An
absent manifest means the feature is not configured; a present manifest with
missing or unexpected content is an error. This also gives garbage collection
something to act on.

### Slot detection

The loader resolves the slot per platform, selected by a mode setting so that
Trident does not hardcode one platform's mechanism. The first mode reads a slot
label from the kernel command line, supplied by a per-slot signed boot artefact.

A label is preferable to a partition UUID because no mapping table has to stay
in sync with a layout that changes. However, a label is an *assertion*: a
packaging error could pair it with the other slot's device. The loader
cross-checks it against the booted `/usr` backing device and refuses to proceed
on a mismatch.

## Change inventory

Sizes are T-shirt estimates covering design and implementation separately. Risk
reflects how much is unknown rather than how much work there is.

### New component

| Change | Design | Impl | Risk | Notes |
| --- | :-: | :-: | :-: | --- |
| `trident-sysext-loader` crate, slot detection and set commit | S | M | Low | std plus `anyhow`; logic is pure given cmdline, store and target, so unit-testable without root or a VM |
| `/etc` layer mounting and lowerdir emission | M | M | Med | Loop-mount in the initrd; interaction with the existing overlay needs care |
| Manifest format and verification | M | S | Low | Decide internal versus API-visible |

### Trident

| Change | Design | Impl | Risk | Notes |
| --- | :-: | :-: | :-: | --- |
| **Atomic extension placement** | S | S | Low | Prerequisite bug fix; live defect today, independent of this RFC |
| `scope` and `extensionStore` API | S | S | Low | Additive, both default |
| `etcLayers` API | M | S | Med | New concept; shape depends on confext migration path |
| Relax `validate_extension_images_locations` | M | S | Med | Must test loader capability, not just A/B |
| Subsystem writes slotted entries per volume | S | M | Low | `provision()` already has the target volume |
| Manifest generation | S | S | Low | |
| Package loader and dracut module in the RPM | S | S | Low | `%files acl` already ships a second binary |
| Schema regeneration | S | S | Low | Mechanical |

### ACL

| Change | Design | Impl | Risk | Notes |
| --- | :-: | :-: | :-: | --- |
| New dracut module to install and run the loader | S | S | Low | `dracut-acl-selinux-toggle` is an existing precedent |
| Patch `bootengine` to add `/etc` lowerdirs | S | M | Med | Third patch alongside two already carried; the mount itself is one line, validation is the work |
| Order the loader before the `/etc` overlay mount | S | S | Low | Same initrd window as `initrd-setup-root` |
| Review `overlaycleanup()` interaction | M | S | Med | It compares only against the distro lower layer, so extension-provided defaults will not auto-prune |
| Package catalog entry | S | S | Low | |
| Initrd size and UKI rebuild | S | S | Med | Re-signing already happens every build; size growth is the open part |

### Not affected

| Component | Why |
| --- | --- |
| systemd / Azure Linux | Nothing. Avoiding a systemd change is the point of this route |
| AgentBaker, AKS RP | No interaction |
| Image Customizer | None unless RFC 0789 lands, and then only at the shared validation |

### Cross-cutting

| Change | Design | Impl | Risk | Notes |
| --- | :-: | :-: | :-: | --- |
| Rollout and capability gating | M | S | Med | Depends on whether the initrd is per-slot; see [Rollout](#rollout) |
| Boot and rollback end-to-end tests | M | L | High | Unsupervised rollback cannot be asserted from unit tests |
| SELinux policy | M | M | High | Extensions are already unsupported with SELinux enforcing |

### Suggested sequencing

1. **Atomic placement fix.** Standalone, lands independently, worth doing
   regardless of this RFC.
2. Loader crate and dracut module, sysext half only, behind no API change.
3. Trident API and the validation relaxation.
4. `/etc` layer half, including the `bootengine` patch. Separable; ships after.
5. Rollout gating, once the per-slot-initrd question is settled.

## Rollout

If the initrd is shared between slots, loader capability is never
slot-dependent and no staged rollout is required.

If it is per-slot, a rollback into a slot whose initrd predates the loader has
no selector, and every slotted extension is absent for that boot. Enabling
`scope: slotted` then requires two phases: deploy a release carrying the loader
and let it reach both slots, then permit slotted configuration. Validation must
verify that capability rather than assume it.

This is an open question, listed below, and it changes the work in the table
above.

## Relationship to RFC 0789

[0789](https://github.com/microsoft/trident/pull/789) carries extension images
in COSI files. The two are orthogonal: 0789 changes where the bytes come from,
this changes where they land and how they follow the slot. Either is useful
alone, and a bundled extension can be slotted.

Both touch `validate_extension_images_locations`, in opposite directions: 0789
tightens it into dynamic validation against the storage graph, this adds a
second way to satisfy it. Whichever lands first should leave the check expressed
as *is this destination slot-scoped*, so the other is an added arm rather than a
rewrite.

## Testing

- Clean install: image in the correct slot directory, selection resolving,
  merged after boot.
- A/B update: the selection repoints and **the merged content differs from the
  previous slot**. A presence-only assertion passes in the inert case and must
  not be used.
- **Unsupervised rollback, with no Trident servicing operation in between**: the
  previous slot's content is merged again. This is the case the design exists
  for.
- `/etc` layer: a slotted layer is visible in `/etc`, `/etc` remains writable,
  and a write to a file the layer provides persists across reboot.
- Loader failure: boot does not proceed silently with a partial set.
- A malformed image in a slot directory is rejected before it is selected.
- Validation: each reject row, plus a shared extension off an A/B volume still
  rejected.
- Compatibility: a configuration with no `scope` places byte-identically to
  today.

## Counter-arguments

- **"Just add the partitions."** Better where possible: the filesystem provides
  the isolation, nothing runs at boot, and capacity is contained. This is for
  layouts that cannot. Directory slotting gives equivalent naming, not
  equivalent failure containment: a store on shared storage can fill and affect
  the running slot, which a dedicated pair cannot.
- **"Put it in `/usr`."** Better still, and unavailable without re-sealing
  dm-verity and re-signing. Upstream also forbids extension storage under
  `/usr`: a `lowerdir=` that is a child of another fails with `-ELOOP`.
- **"Use the ESP."** Genuinely slot-scoped via `<uki>.efi.extra.d/`, but ESPs
  are 128 to 256 MiB against payloads in the tens to hundreds of megabytes, and
  `systemd-stub` places them in the initrd.
- **"Backport mutable confexts."** It exists, and it is 1310 lines across five
  files of someone else's component. This route reaches the same place without
  carrying that, and converges on native confexts later at zero payload cost.
- **"Doubles storage."** It does. The A/B volume route doubles it too, with the
  allocation fixed at install.

### Rejected mechanisms

- **`.v` versioned directories.** They select the highest version and stop, with
  no fallback when that image fails OS matching, so they cannot hold per-slot
  copies. Verified empirically; Flatcar reached the same conclusion.
- **`systemd-tmpfiles`.** No specifier identifies the booted device, partition
  or slot, symlink targets are literal text plus specifier expansion, and the
  main pass is ordered after the merge.
- **A systemd generator.** Generators run sandboxed with most of the filesystem
  read-only, and `/var` is not mounted when they run.
- **Reordering the sysext merge ahead of the `/etc` overlay.** Exhausts the
  filesystem stacking budget and breaks confexts outright.
- **Repair after boot.** Any agent running at `multi-user.target` is far too
  late to influence the merge.

### Prior art

Three shipping systems converge on recomputing the selection at boot rather than
persisting it:

- **Flatcar**, whose layout is closest to the first consumer, stamps the version
  into the filename on shared storage and rewrites the activating symlink in the
  initrd from the booted `/usr`'s `os-release`. It has since moved extension
  mounting wholesale into the initrd and deleted its late-merge workaround.
- **ChromeOS** stores `dlc_a`/`dlc_b` on shared stateful storage and derives the
  slot from the booted partition number on every service start. It clears the
  inactive slot's validity marker *before* writing and re-sets it after, so a
  rollback into a half-written slot finds an honest failure rather than a
  plausible file. Worth adopting.
- **Fedora's `sysexts-manager`** filters candidates by the running `VERSION_ID`
  and symlinks the winner into `/run/extensions`.

The counter-example is instructive: Fedora CoreOS activates sysexts statically
across deployments, and its documentation records that after a major rebase the
extensions silently stop matching and are not loaded.

## Open questions

1. **Is the initrd per-slot or shared?** Decides whether a staged rollout is
   needed at all, and changes the estimates above.
2. **Whole-directory selection or a prepared set?** Both commit atomically. The
   former is simpler; the latter allows shared and slotted extensions to share a
   search path.
3. **Manifest format**, and whether it is Trident-internal or API-visible.
4. **Retention.** Nothing prunes a slot directory when an entry leaves the
   configuration. Reconciling in `provision()` conflicts with keeping the
   previous slot's files for rollback.
5. **Default store path.** `/var/trident/extensions` against
   `/var/lib/trident/extensions`; the datastore already lives under the latter.
6. **Measured boot.** The selected image is outside the UKI's measurements, so
   identical measurements can yield different `/usr` contents. Whether that is
   acceptable depends on the platform's attestation claims.
7. **SELinux.** Extensions are already unsupported with SELinux enforcing, which
   is a precondition this RFC does not resolve.

## Future possibilities

- **Native confext mounting.** On systemd 256 or later, the loader places the
  selected payload in a confext search path and `systemd-confext --mutable=yes`
  mounts it, replacing the custom lowerdir handling. The loader still performs
  selection, and the payloads do not change provided they declare `ID=_any`.
  Azure Linux 4.0 ships systemd 258.4.
- **Content matching** on platforms with systemd v256 or later and a
  build-granular `VERSION_ID`, which needs no loader at all.
- **Initrd-phase sysext merging**, as Flatcar now does, for extensions that must
  be present before unit enumeration.
- Per-volume scoping for other Trident-managed state that is shared today.
