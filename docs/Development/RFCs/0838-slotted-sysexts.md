# 0838 Slotted Extension Storage

- Date: 2026-10-12
- RFC PR: [microsoft/trident#838](https://github.com/microsoft/trident/pull/838)
- Issue: TBD

## Summary

Trident requires every extension destination to sit on an A/B volume when A/B is
configured. That is one way to make an extension follow the slot, and it costs a
partition pair per extension directory.

This adds a second. An extension marked `scope: slotted` is stored per volume
under a Trident-owned store. A small boot-time component selects the booted
slot's copy into `/run/extensions` before the merge runs. Slot-scoping becomes a
property of the extension rather than of the partition table.

## Motivation and Goals

`validate_extension_images_locations` (`config/host/mod.rs:218`) rejects any
extension directory that does not resolve to a device with A/B capabilities,
raising `ExtensionImageNotOnABVolume`. The reasoning is sound: extensions on
shared storage change the running slot immediately and do not roll back.

The cost is that the only way to satisfy it is a dedicated A/B volume pair.
Trident's own reference configuration
(`tests/e2e_tests/trident_configurations/root-verity/trident-config.yaml`)
spends four partitions and 2.4 GB on it.

That price buys only slot-scoping, and it is charged in the currency hardest to
change:

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
platform it would need no new code at all.

It fails for two independent reasons, either of which is sufficient:

1. **Deploying one image under two filenames requires systemd v256.**
   `path_extract_image_name()` truncates an image name at the first `_` or `+`
   (`src/basic/os-util.c`), so `platform_1.raw` and `platform_2.raw` can share a
   single inner `extension-release.platform`. That function does not exist
   before v256; it was added by commit `37543971` (2024-03-28). On earlier
   systemd each copy needs its own `extension-release`, so a signed image must
   be repacked per slot. The `user.extension-release.strict` xattr relaxes the
   name correspondence but must be set inside the image at build time.
2. **The OS version must actually differ between slots.** Matching uses `ID=`,
   then `SYSEXT_LEVEL=` when defined on both sides, otherwise `VERSION_ID=`. A
   platform whose `VERSION_ID` is coarser than its build cadence, or which can
   deploy the same image to both slots, puts identical values in both slots and
   both copies then match.

Both conditions hold on the first intended consumer. Content matching remains
the better mechanism where a platform satisfies both, and this design does not
preclude adopting it there.

### Goals

Slot-scoped extensions without a dedicated volume pair; rollback restores the
previous set with no servicing operation; no new partitions, no sealed-`/usr`
change, no re-signing; existing configurations unaffected.

### Non-goals

Changing `systemd-sysext` matching semantics. Removing the A/B volume route,
which stays better where a pair is affordable, since the filesystem provides
the isolation and Trident does nothing. Extensions on the ESP.

## Scope

Sysexts and confexts declared under `os.sysexts` and `os.confexts`, on clean
install and A/B update. Runtime update does not change slot and is unaffected.

**Slotted extensions carry `/usr` and `/opt` content only.** Contributing to
`/etc` is out of scope for this RFC. On a platform that assembles `/etc` as an
overlay whose lowerdir lives inside `/usr`, a sysext can only affect `/etc` if
it is merged before that overlay is built. Reordering the merge ahead of it
would place the `/etc` overlay on top of the `/usr` overlay, consuming the
kernel's entire filesystem stacking budget: `FILESYSTEM_MAX_STACK_DEPTH` is 2
(`include/linux/fs.h`), enforced in `fs/overlayfs/super.c`, and an overlay's
depth is the maximum of its layers plus one. The `/etc` overlay would reach
depth 2 and any confext merged onto `/etc` afterwards would reach 3 and fail.

The durable fix is to stop sourcing `/etc` defaults from inside `/usr`, which is
a platform change rather than a Trident one.

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
  # Default /var/trident/sysexts. Must be on storage an A/B update does not
  # replace, must not be a systemd-sysext search path, and must be on a
  # filesystem available at local-fs.target.
  extensionStore: /mnt/data/trident/sysexts

  sysexts:
    # unchanged, and still the default
    - url: https://example.com/tooling.raw
      sha384: <hash>

    # new
    - url: https://example.com/platform.raw
      sha384: <hash>
      scope: slotted
```

| `scope` | Where the bytes live | Where it is activated |
| --- | --- | --- |
| `shared` (default) | At `path`. | At `path`. Today's behaviour. |
| `slotted` | `<extensionStore>/{a,b}/{name}.raw` | `/run/extensions/`, by the loader |

A slotted extension always surfaces in `/run/extensions`, so `path` does not
apply to it and is rejected.

### The validation change

`validate_extension_images_locations` is relaxed, not removed. Its requirement
becomes *the destination is slot-scoped*, satisfied either way:

| Configuration | Rule |
| --- | --- |
| No `abUpdate` | No check, as today. |
| `abUpdate`, `scope: shared` | `path` must be on an A/B volume. Unchanged. |
| `abUpdate`, `scope: slotted` | `path` need not be; the store plus the loader provide the scoping. |

This is the substantive behavioural change. It is only safe where the platform
actually provides a loader, so validation must check a **platform capability**,
not merely that A/B is configured. See [Rollout](#rollout).

### Validation

| Condition | Result |
| --- | --- |
| `scope` absent | `shared`. Identical to today. |
| `scope: slotted` with `path` | Reject. The destination is loader-owned. |
| `scope: slotted`, no `abUpdate` | Reject, naming the absent configuration. |
| `scope: slotted`, no loader capability | Reject. See [Rollout](#rollout). |
| `extensionStore` inside a search path | Reject: both slots' copies would be discovered. |
| `extensionStore` on an A/B volume | Reject: it must outlive the slot it describes. |
| `extensionStore` not available at `local-fs.target` | Reject: the loader runs before anything later. |
| Two slotted entries resolving to the same basename | Reject: they would collide in `/run/extensions`. |

### Backward compatibility

Both fields default, so existing Host Configurations parse and behave
identically. The relaxed check is strictly more permissive, so anything valid
today stays valid. The schema gains two optional properties.

### Alternatives rejected

- **Sentinel `path`** (`<store>/<slot>/x.raw`): leaks internal layout, invites a
  literal `a` or `b`.
- **Separate `os.slottedSysexts` list**: duplicates the type and its validation.
- **Infer from `extension-release`**: inside the image, so unavailable at static
  validation, and it removes the operator's choice to slot an `ID=_any`
  extension deliberately.

## Implementation

```
/var/trident/sysexts/
├── a/platform.raw
└── b/platform.raw

/run/extensions/
└── platform.raw -> /var/trident/sysexts/a/platform.raw    # recreated each boot
```

The store is outside every search path, so its contents are invisible to
`systemd-sysext` until selected. Both slot directories persist across an update,
so rollback needs no re-download.

Symlinks are followed: `image_discover` opens each search path with
`chase_and_opendir(path, root, CHASE_PREFIX_ROOT, ..)` and stats entries with
`flags = 0` on a running system, with an upstream comment explicitly permitting
symlinks into the search path (`src/shared/discover-image.c`).

### Servicing writes the store, and must write it atomically

The extensions subsystem already runs on clean install and A/B update via
`provision()`, documented as "migrate state from A-partition to B-partition (or
vice versa)". For a slotted entry it resolves the target volume with
`EngineContext::get_ab_update_volume()`, already used to select the per-slot
verity addon, and writes into `<store>/<target>/`.

Placement must be atomic. A malformed `.raw` is not skipped like a version
mismatch: `image_read_metadata` errors propagate in
`image_discover_and_read_metadata` (`src/sysext/sysext.c`) before any per-image
tolerance applies, so **one bad image blocks every extension on the host**.
Directory-type images are tolerated; `.raw` are not.

Trident today falls back to a non-atomic `fs::copy` when `rename` fails across a
filesystem boundary (`crates/trident/src/subsystems/extensions/mod.rs:525-540`).
Staging must move to a temporary file on the destination's own filesystem,
verified and `fsync`ed, then renamed. This is a prerequisite, not a
consequence.

### Selection happens at boot

Selection is recomputed on every boot from the slot that actually booted. This
is what makes unsupervised rollback correct: when firmware falls back on its
own, no Trident code runs, so any decision written during servicing would be
stale with nothing to correct it.

`/run` is tmpfs, so a stale pointer cannot be represented across a boot.

The loader resolves the booted slot, then exposes the matching slot directory's
contents in `/run/extensions`. Its unit:

```ini
[Unit]
Description=Select Trident system extensions for the booted slot
DefaultDependencies=no
After=local-fs.target
Before=systemd-sysext.service
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/usr/bin/trident-sysext-loader
[Install]
WantedBy=sysinit.target
```

`DefaultDependencies=no` is required; the implicit `After=sysinit.target` would
place it after the merge and create a cycle.

`Before=systemd-sysext.service` is the correct edge even on platforms that do
not enable that unit directly. Flatcar-derived systems wire
`ensure-sysext.service` into `sysinit.target.wants` instead, but that unit
carries `BindsTo=systemd-sysext.service` and `After=systemd-sysext.service`: it
runs *after* the merge, and exists to `daemon-reload` and restart targets so
that units shipped inside an extension are picked up. Ordering before it would
therefore permit running after the merge.

This works because systemd evaluates conditions in `unit_start()`
(`src/core/unit.c`), not at transaction creation, so the loader populates
`/run/extensions` before the merge unit's own `ConditionDirectoryNotEmpty=` is
tested. The same fact rules out an `ExecStartPre=` drop-in, since that condition
gates the whole unit and would skip it while the directories are still empty.

### Failing closed

`WantedBy=` creates a `Wants=`, which `systemd.unit(5)` states has no effect on
the transaction when it fails, and `Before=` carries no requirement. The unit
above therefore does **not** fail closed on its own: a loader failure would
leave boot to continue with extensions absent or partially present.

The merge unit must carry `Requires=trident-sysext-loader.service` via a
drop-in. This must be a deliberate requirement edge rather than forcing
emergency mode, because the store is on shared storage and a storage fault must
not be able to fail both slots and defeat A/B rollback.

### Selection is a set, not a sequence

Creating N symlinks exposes a prefix of the intended set if it fails partway,
which matters where a driver and its userspace libraries must move together.
The loader must therefore commit the set atomically: prepare the complete set
in a staging directory under `/run` and rename it into place, or select the
whole directory at once.

Selecting the whole directory is simpler but means everything at that search
path comes from one slot, so a shared extension can no longer sit beside a
slotted one there. That trade should be settled before implementation.

### Slot detection

The loader resolves the slot per platform, selected by a `LOADER_MODE`
environment variable so that Trident does not hardcode one platform's
mechanism. The first mode reads a slot label from the kernel command line,
supplied by a per-slot signed boot artefact.

A label is preferable to a partition UUID because no mapping table has to stay
in sync with a layout that changes. However, a label is an *assertion*: a
packaging error could pair it with the other slot's device. The loader should
cross-check it against the booted `/usr` backing device and refuse to proceed on
a mismatch.

Platform-specific conditions belong in platform-specific units. A single unit
carrying `ConditionKernelCommandLine=` for one platform's parameter would
prevent every other mode from ever running, since the condition is evaluated
before the binary reads `LOADER_MODE`.

### The store needs a manifest

Globbing `*.raw` cannot distinguish "no extensions configured" from "storage
lost", "mount failed" or "update interrupted", and treating all of those as
success produces a quietly extension-less host.

Each slot directory should carry a manifest naming the expected images and their
hashes. An absent manifest means the feature is not configured; a present
manifest with missing or unexpected content is an error. This also gives
garbage collection something to act on, which globbing does not.

## Rollout

The loader ships in `/usr` and therefore rides the A/B swap. A rollback into a
slot whose `/usr` predates the loader has no selector, and every slotted
extension is absent for that boot.

Enabling `scope: slotted` therefore requires two phases: first deploy a release
carrying the loader and let it reach both slots, then permit slotted
configuration. Validation must verify that capability rather than assume it,
which is why the relaxed check above tests for a loader and not merely for A/B.

## Relationship to RFC 0789

[0789](https://github.com/microsoft/trident/pull/789) carries extension images
in COSI files. The two are orthogonal: 0789 changes where the bytes come from,
this changes where they land and how they follow the slot. Either is useful
alone, and a bundled extension can be slotted.

Both touch `validate_extension_images_locations`, in opposite directions: 0789
tightens it into dynamic validation against the storage graph, this adds a
second way to satisfy it. Whichever lands first should leave the check expressed
as *is this destination slot-scoped*, so the other is an added arm rather than a
rewrite. 0789's rollback argument, that extensions revert because the slot's
filesystem reverts, is the A/B volume mechanism and holds unchanged for
`scope: shared`.

## Testing

- Clean install: image in the correct slot directory, `/run/extensions` link
  resolving, merged after boot.
- A/B update: the selection repoints and **the merged content differs from the
  previous slot**. A presence-only assertion passes in the inert case and must
  not be used.
- **Unsupervised rollback: with no Trident servicing operation in between**, the
  previous slot's content is merged again. This is the case the design exists
  for.
- Loader failure: boot does not proceed silently with a partial set.
- Ordering: the selection lands before the merge, asserted on the first boot
  after an update rather than on a steady-state boot.
- A malformed image in a slot directory is rejected before it reaches
  `/run/extensions`.
- Validation: each reject row, plus a shared extension off an A/B volume still
  rejected.
- Compatibility: a configuration with no `scope` places byte-identically to
  today.

## Counter-arguments

- **"Just add the partitions."** Better where possible: the filesystem provides
  the isolation, Trident does nothing at boot, and capacity is contained. This
  is for layouts that cannot. Directory slotting gives equivalent naming, not
  equivalent failure containment: a store on shared storage can fill and affect
  the running slot, which a dedicated pair cannot.
- **"Do it in the initrd."** Flatcar does exactly this and it is the stronger
  position in principle, since the merge then precedes unit enumeration and the
  `/etc` overlay. It requires rebuilding and re-signing the UKI, which is a
  delivery cost rather than a correctness argument, and it does not fit a
  platform that cannot modify its initrd. Worth revisiting for extensions needed
  before `sysinit.target`, which this design cannot serve.
- **"Put it in `/usr`."** Better still, and unavailable without re-sealing
  dm-verity and re-signing. Upstream also forbids extension storage under
  `/usr`: a `lowerdir=` that is a child of another fails with `-ELOOP`.
- **"Use the ESP."** Genuinely slot-scoped via `<uki>.efi.extra.d/`, but ESPs
  are 128 to 256 MiB against payloads in the tens to hundreds of megabytes, and
  `systemd-stub` places them in the initrd.
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
- **Repair after boot.** Any agent running at `multi-user.target` is far too
  late to influence the merge.

### Prior art

Three shipping systems converge on recomputing the selection at boot rather than
persisting it:

- **Flatcar**, whose layout is closest to the first consumer, stamps the version
  into the filename on shared storage and rewrites the activating symlink in the
  initrd from the booted `/usr`'s `os-release`.
- **ChromeOS** stores `dlc_a`/`dlc_b` on shared stateful storage and derives the
  slot from the booted partition number on every service start. It also clears
  the inactive slot's validity marker *before* writing and re-sets it after, so
  a rollback into a half-written slot finds an honest failure rather than a
  plausible file. That is worth adopting.
- **Fedora's `sysexts-manager`** filters candidates by the running `VERSION_ID`
  and symlinks the winner into `/run/extensions`.

The counter-example is instructive: Fedora CoreOS activates sysexts statically
across deployments, and its documentation records that after a major rebase the
extensions silently stop matching and are not loaded.

## Open questions

1. **Whole-directory selection or a prepared set?** Both commit atomically. The
   former is simpler; the latter allows shared and slotted extensions to share a
   search path.
2. **Manifest format and whether it is Trident-internal or part of the API.**
3. **Retention.** Nothing prunes a slot directory when an entry leaves the
   configuration. Reconciling in `provision()` conflicts with keeping the
   previous slot's files for rollback.
4. **Default store path.** `/var/trident/sysexts` against
   `/var/lib/trident/sysexts`; the datastore already lives under the latter.
5. **Confexts.** The same mechanism applies with different search paths, but
   `/etc` content is out of scope here, so whether confexts are in scope at all
   needs deciding.
6. **SELinux.** Extensions are already unsupported with SELinux enforcing.
   Slotting does not change that, but loader-created symlinks in `/run` need
   labelling, and 0789 proposes moving that check to every servicing type.
7. **Measured boot.** The selected image is outside the UKI's measurements, so
   identical measurements can yield different `/usr` contents. Whether that is
   acceptable depends on the platform's attestation claims.

## Future possibilities

- **Initrd-phase selection**, for extensions that must be merged before unit
  enumeration or before an `/etc` overlay is assembled.
- **Content matching** on platforms with systemd v256 or later and a
  build-granular `VERSION_ID`, which needs no loader at all.
- Per-volume scoping for other Trident-managed state that is shared today.
