# 0838 Slotted Extension Storage

- Date: 2026-10-14
- RFC PR: [microsoft/trident#838](https://github.com/microsoft/trident/pull/838)
- Builds on: [RFC 0789](https://github.com/microsoft/trident/pull/789)

## Summary

[RFC 0789](https://github.com/microsoft/trident/pull/789) carries extension
images inside the COSI, so an OS-coupled extension ships and rolls out with the
OS. It leaves one question open: such an extension still has to land somewhere
that follows the A/B slot, and today the only way to satisfy that is a dedicated
A/B partition pair.

This RFC answers it. A bundled extension can request Trident-managed, per-slot
storage, and a boot-time component selects the booted slot's copy before
anything consumes it. It also supplies `/etc` layers, which have no viable
mechanism on systemd 255.

An extension bundled in the COSI is already tied to the OS image it shipped
with. Slotted placement is the natural destination for it, and the two features
are expected to be used together in the common case. An independently hosted
extension fetched over OCI and placed per slot is the unusual combination, not
the default.

## Motivation and Goals

0789 establishes that OS-coupled extensions belong in the COSI. It also
establishes, in [Destination
Validation](https://github.com/microsoft/trident/pull/789), that a bundled image
written to shared storage changes the running slot immediately and defeats
rollback, so placement must be slot-scoped.

Trident has exactly one mechanism for that today, and
`validate_extension_images_locations` (`config/host/mod.rs:218`) enforces it: the
destination must resolve to a device with A/B capabilities. The only way to
satisfy it is a dedicated A/B volume pair. Trident's own reference configuration
(`tests/e2e_tests/trident_configurations/root-verity/trident-config.yaml`)
spends four partitions and 2.4 GB on exactly this.

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

A tempting alternative is to place both slots' copies in one search directory
and let `systemd-sysext` merge whichever matches the booted OS. Version
mismatches are skipped quietly, so this very nearly works, and on a suitable
platform it needs no new code at all.

It fails for two independent reasons, either sufficient on its own:

1. **Deploying one image under two filenames requires systemd v256.**
   `path_extract_image_name()` truncates an image name at the first `_` or `+`
   (`src/basic/os-util.c`), letting two copies share one inner
   `extension-release`. That function arrived in commit `37543971` (2024-03-28).
   Earlier systemd requires each copy to carry its own, so a signed image must
   be repacked per slot.
2. **The OS version must actually differ between slots.** Matching uses `ID=`,
   then `SYSEXT_LEVEL=` when defined on both sides, otherwise `VERSION_ID=`. A
   platform whose `VERSION_ID` is coarser than its build cadence, or which can
   deploy the same image to both slots, puts identical values in both and both
   copies match.

Both conditions hold on the first intended consumer. Content matching remains
the better mechanism where a platform satisfies both; see
[Future possibilities](#future-possibilities).

### Goals

Give bundled extensions a slot-scoped destination without a dedicated volume
pair; restore the previous set on rollback with no servicing operation; provide
a workable `/etc` story on systemd 255; require no new partitions, no
sealed-`/usr` change and no modification to systemd.

### Non-goals

Changing `systemd-sysext` matching semantics. Replacing the A/B volume route,
which stays better where a pair is affordable. Extensions on the ESP. Anything
0789 already covers, including the effective set, the deployed inventory and
extension-only updates.

## Dependencies

**RFC 0789 is a prerequisite.** This RFC adds a placement policy to the model
0789 defines and does not restate it. In particular it relies on:

| From 0789 | Used here as |
| --- | --- |
| `extensions.sysexts` / `extensions.confexts` in COSI 1.3 | The delivery mechanism for slotted content |
| `(kind, id)` as stable extension identity | The store key, instead of a filename |
| Deployed Extension Inventory in Host Status | The record of what is deployed per slot; this RFC adds no separate manifest |
| Effective set computed from COSI plus Host Configuration | The set the loader selects from |
| Resolved-placement destination validation | The hook where the slotted arm is added |
| Atomic staging on the destination filesystem | A prerequisite shared by both RFCs |
| SELinux check moved into validation for every servicing type | Inherited unchanged |

0789's prerequisites apply transitively, including the `set_up_extensions`
replacement bug and durable staged payloads.

Bundled extensions under
[disk streaming](../../Explanation/Disk-Streaming.md) are refused by 0789; that
restriction is inherited.

## Design

### Selection is recomputed every boot, never persisted

When firmware falls back to the previous slot on its own, no Trident code runs.
Anything written during servicing would be stale with nothing to correct it, so
the selection must derive from what actually booted.

Verified: Trident's A/B rollback path invokes no subsystems.
`rollback::validate_boot` only reconciles Host Status, and
`manual_rollback::stage_rollback` touches only the pcrlock policy. Only the
runtime path passes `&mut subsystems`.

### Both halves run in the initrd

The `/etc` half has no choice. Where `/etc` is assembled as an overlay whose
lowerdir lives inside `/usr`, anything contributing to it must act before that
overlay is mounted, which happens in the initrd.

The sysext half rides along, because `/run` is bind-moved across switch-root
(`src/shared/switch-root.c`: `{ "/run", MS_BIND, MS_BIND|MS_REC }`). Symlinks
written to `/run/extensions` in the initrd are still present when
`systemd-sysext` merges in the real root. Their targets are written as
post-switch-root paths, so they are dangling in the initrd and correct after.

Unifying is better than running them separately: the slot is resolved **once**,
so the sysext set and the `/etc` layer cannot disagree about which slot they
came from. It also removes complexity a real-root systemd unit would carry:

| Problem with a real-root unit | In the initrd |
| --- | --- |
| `WantedBy=` creates a `Wants=`, which `systemd.unit(5)` states cannot fail the transaction, so the unit does not fail closed | No unit; failing in dracut is native |
| Ordering is subtle: `ensure-sysext.service` carries `BindsTo=` and `After=systemd-sysext.service`, so it runs *after* the merge and is the wrong anchor | Nothing to order against |
| A platform-specific `ConditionKernelCommandLine=` would block every other mode | No unit condition |
| Unit enablement needs a preset or scriptlet | No unit to enable |
| Store must be available at `local-fs.target` | `/sysroot` is mounted by definition |

A real-root unit remains the fallback for platforms that cannot modify their
initrd, so the mode selector chooses *where* the loader runs as well as how it
detects the slot.

### Why `/etc` payloads are confext images

`systemd-confext` is the natural mechanism and is unusable here: on systemd 255
it mounts `/etc` read-only. Mutable overlays arrived in v256; Flatcar's backport
is 1310 lines across five files, and modifying systemd is out of scope.

Keep the **format** and replace the **mount**. Payloads are confext-format DDIs
carrying `/etc/extension-release.d/extension-release.<name>`
(`src/basic/os-util.c`). The loader mounts them read-only and adds them as
additional lowerdirs to the existing `/etc` overlay, whose upperdir is
unchanged, so `/etc` stays mutable.

Preferred over tarballs or plain trees because:

- **It is forward-compatible.** The payload is already a valid confext. Azure
  Linux 4.0 ships systemd 258.4, where the identical artefact is handed to
  native `systemd-confext --mutable=yes` and the custom mounting is deleted.
  Two things do not change at that transition: the loader remains, because
  systemd has no concept of A/B slots and selection stays the platform's job;
  and native mounting reintroduces `extension-release` matching that the custom
  lowerdir mount bypasses.
- **It reuses everything.** Same build and signing pipeline as sysexts, same
  0789 delivery path, verity available by DDI convention.
- **It is atomic.** A loop mount either appears or does not. A tarball
  extraction can half-fail and pays extraction cost on every boot.
- **SELinux labels travel in the filesystem image**, not in tar metadata.

#### Payloads must declare `ID=_any`

Because slot selection is the platform's job and not systemd's, the payload must
not additionally be subject to OS-version matching. A payload declaring a
concrete `ID=` would merge under the custom mount, which ignores matching, then
silently stop merging the day a platform switches to native confext mounting.
That is the failure mode this RFC exists to prevent, reappearing at the point of
transition.

`ID=_any` bypasses matching on every systemd version. Selection remains with the
loader, which is what slot-scoping requires anyway, and the move to native
mounting changes no artefact. Deciding this late would mean rebuilding every
payload, so it is specified here. 0789 already states that `ID=_any` must not be
warned on, since it declares compatibility rather than delivery method.

Adding lowerdirs does not increase filesystem stacking depth, because depth is
the maximum of the layers plus one and every lowerdir sits on a plain
filesystem. *Reordering* the sysext merge ahead of the `/etc` overlay would
instead place `/etc` on top of the `/usr` overlay, reaching depth 2, and any
confext merged afterwards would reach 3 and fail.
`FILESYSTEM_MAX_STACK_DEPTH` is 2 (`include/linux/fs.h`), enforced in
`fs/overlayfs/super.c`.

### Selection is a set, not a sequence

Creating N symlinks exposes a prefix of the intended set if it fails partway,
which matters where a driver and its userspace libraries must move together.
The loader prepares the complete set and commits it with a single rename.

## Public API Design

### Answering 0789's open question

0789 asks how a COSI entry requests slotted placement, and states that omitting
`path` must not opt in. The answer is an explicit `scope`, spelled identically
on both sources so one concept has one name.

**COSI metadata**, extending 0789's `ExtensionImage`:

```json
{
  "id": "gpu-driver",
  "image": { "path": "images/extensions/gpu-driver.rawzst", "...": "..." },
  "scope": "slotted"
}
```

**Host Configuration**, extending `Extension`:

```yaml
os:
  # Root of the per-volume extension store.
  # Default /var/trident/extensions. Must be on storage an A/B update does not
  # replace, and must not be a systemd-sysext search path.
  extensionStore: /mnt/data/trident/extensions

  sysexts:
    - url: https://example.com/tooling.raw   # unchanged, still the default
      sha384: <hash>
    - url: https://example.com/platform.raw
      sha384: <hash>
      scope: slotted

  # confext-format images contributed as /etc overlay layers
  etcLayers:
    - url: https://example.com/platform-etc.raw
      sha384: <hash>
      scope: slotted
```

| `scope` | Where the bytes live | Where it is activated |
| --- | --- | --- |
| `shared` (default) | At `path`. | At `path`. Today's behaviour. |
| `slotted` | `<extensionStore>/{a,b}/`, keyed by `(kind, id)` | `/run/extensions`, or as an `/etc` lowerdir |

`scope` and `path` are mutually exclusive: a slotted extension surfaces in a
loader-owned location, so a caller-supplied destination would be silently
ignored. This satisfies 0789's requirement that placement never change
implicitly.

`etcLayers` is a separate list rather than a `scope` on `os.confexts` because
the two are activated by different mechanisms, only one of which is
`systemd-confext`. The COSI side needs no equivalent split: a confext entry with
`scope: slotted` is an `/etc` layer.

### The validation change

`validate_extension_images_locations` is relaxed, not removed. Its requirement
becomes *the destination is slot-scoped*, satisfied by either mechanism. This is
the arm 0789's Destination Validation anticipates:

| Configuration | Rule |
| --- | --- |
| No `abUpdate` | No check, as today. |
| `abUpdate`, `scope: shared` | Destination must be on an A/B volume. Unchanged. |
| `abUpdate`, `scope: slotted` | Satisfied by the store, **only when loader capability is present**. |

0789 states the same constraint from its side: slotted storage qualifies only
with loader capability, and omitting `path` must not bypass the A/B check.

### Validation

| Condition | Result |
| --- | --- |
| `scope` absent | `shared`. Identical to today. |
| `scope: slotted` with `path` | Reject. The destination is loader-owned. |
| `scope: slotted`, no `abUpdate` | Reject, naming the absent configuration. |
| `scope: slotted`, no loader capability | Reject. See [Rollout](#rollout). |
| `extensionStore` inside a search path | Reject: both slots' copies would be discovered. |
| `extensionStore` on an A/B volume | Reject: it must outlive the slot it describes. |
| `etcLayers` entry without confext metadata | Reject at deploy time, after mount. |
| `etcLayers` entry without `ID=_any` | Reject; see above. |

Duplicate `(kind, id)` and destination collisions are already covered by 0789
and are not restated.

### Backward compatibility

All new fields default, so existing Host Configurations and 1.3 COSI files
without `scope` behave identically. The relaxed check is strictly more
permissive, so anything valid today stays valid.

## Implementation

```
<extensionStore>/
├── a/
│   ├── sysext/<id>.raw
│   └── etc/<id>.raw
└── b/ ...

/run/extensions/<name>.raw -> <extensionStore>/a/sysext/<id>.raw
/etc overlay lowerdir list += <mounted etc/<id>.raw>
```

Keyed by `(kind, id)` from 0789 rather than by filename, so replacement and
comparison use the same identity the inventory does. The store is outside every
search path, so its contents are invisible to `systemd-sysext` until selected.
Both slot directories persist across an update, so rollback needs no
re-download.

### Servicing

0789's extensions subsystem already computes the effective set and places
images. For a slotted entry the destination becomes `<store>/<target>/`, with
the target volume from `EngineContext::get_ab_update_volume()`, already used to
select the per-slot verity addon.

What is deployed per slot is recorded in 0789's Deployed Extension Inventory.
This RFC adds no separate manifest: the inventory already carries kind, ID,
name, resolved placement, source and source hash, which is what the loader needs
to distinguish "nothing configured" from "storage lost".

Atomic placement is a shared prerequisite. A malformed `.raw` is not skipped like
a version mismatch: `image_read_metadata` errors propagate in
`image_discover_and_read_metadata` (`src/sysext/sysext.c`) before any per-image
tolerance applies, so one bad image blocks every extension on the host.

### Slot detection

The loader resolves the slot per platform, selected by a mode setting so Trident
does not hardcode one platform's mechanism. The first mode reads a slot label
from the kernel command line, supplied by a per-slot signed boot artefact.

A label is preferable to a partition UUID because no mapping table has to stay
in sync with a layout that changes. However, a label is an *assertion*: a
packaging error could pair it with the other slot's device. The loader
cross-checks it against the booted `/usr` backing device and refuses to proceed
on a mismatch.

## Change inventory

T-shirt estimates covering design and implementation separately. Risk reflects
how much is unknown rather than how much work there is. Rows delivered by 0789
are excluded.

### New component

| Change | Design | Impl | Risk | Notes |
| --- | :-: | :-: | :-: | --- |
| `trident-sysext-loader` crate, slot detection and set commit | S | M | Low | std plus `anyhow`; pure given cmdline, store and inventory, so unit-testable without root or a VM |
| `/etc` layer mounting and lowerdir emission | M | M | Med | Loop-mount in the initrd; interaction with the existing overlay needs care |

### Trident

| Change | Design | Impl | Risk | Notes |
| --- | :-: | :-: | :-: | --- |
| `scope` on COSI `ExtensionImage` and HC `Extension` | S | S | Low | Additive; answers 0789's open question |
| `extensionStore` and `etcLayers` API | M | S | Med | `etcLayers` shape depends on the confext migration path |
| Relax `validate_extension_images_locations` | M | S | Med | Adds the slotted arm to 0789's resolved-placement validation |
| Slotted destination in the extensions subsystem | S | S | Low | 0789 already resolves placement; this adds one destination policy |
| Loader capability detection | M | S | Med | Needed by both RFCs' validation |
| Package loader and dracut module in the RPM | S | S | Low | `%files acl` already ships a second binary |
| Schema regeneration | S | S | Low | Mechanical |

### ACL

| Change | Design | Impl | Risk | Notes |
| --- | :-: | :-: | :-: | --- |
| New dracut module to install and run the loader | S | S | Low | `dracut-acl-selinux-toggle` is an existing precedent |
| Patch `bootengine` to add `/etc` lowerdirs | S | M | Med | Third patch alongside two already carried; the mount is one line, validation is the work |
| Order the loader before the `/etc` overlay mount | S | S | Low | Same initrd window as `initrd-setup-root` |
| Review `overlaycleanup()` interaction | M | S | Med | It compares only against the distro lower layer, so extension-provided defaults will not auto-prune |
| Package catalog entry | S | S | Low | |
| Initrd size and UKI rebuild | S | S | Med | Re-signing already happens every build; size growth is the open part |

### Not affected

| Component | Why |
| --- | --- |
| systemd / Azure Linux | Nothing. Avoiding a systemd change is the point of this route |
| AgentBaker, AKS RP | No interaction |
| Image Customizer | Covered by 0789, which already requires a writer emitting the section |

### Cross-cutting

| Change | Design | Impl | Risk | Notes |
| --- | :-: | :-: | :-: | --- |
| Rollout and capability gating | M | S | Med | Depends on whether the initrd is per-slot |
| Boot and rollback end-to-end tests | M | L | High | Unsupervised rollback cannot be asserted from unit tests |
| SELinux policy | M | M | High | Extensions are already unsupported with SELinux enforcing |

### Suggested sequencing

1. 0789 steps 1 and 2: prerequisites, COSI 1.3, and reading and installing
   bundled extensions.
2. Loader crate and dracut module, sysext half only, behind no API change.
3. `scope` plumbing and the validation relaxation.
4. `/etc` layer half, including the `bootengine` patch. Separable; ships after.
5. Rollout gating, once the per-slot-initrd question is settled.

The atomic-placement fix is a prerequisite of both RFCs and should land first
and independently; it is a live defect today.

## Rollout

If the initrd is shared between slots, loader capability is never
slot-dependent and no staged rollout is required.

If it is per-slot, a rollback into a slot whose initrd predates the loader has
no selector, and every slotted extension is absent for that boot. Enabling
`scope: slotted` then requires two phases: deploy a release carrying the loader
and let it reach both slots, then permit slotted configuration. Validation must
verify that capability rather than assume it, which is also what 0789 requires
of its destination validation.

## Testing

Beyond 0789's coverage:

- A bundled extension with `scope: slotted`: placed in the correct slot
  directory, selected at boot, merged.
- A/B update: the selection repoints and **the merged content differs from the
  previous slot**. A presence-only assertion passes in the inert case and must
  not be used.
- **Unsupervised rollback, with no Trident servicing operation in between**: the
  previous slot's content is merged again. This is the case the design exists
  for.
- `/etc` layer: visible in `/etc`, `/etc` remains writable, and a write to a
  file the layer provides persists across reboot.
- Loader failure: boot does not proceed silently with a partial set.
- Validation: each reject row, plus a shared extension off an A/B volume still
  rejected, and `scope: slotted` rejected without loader capability.

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

### Rejected mechanisms

- **`.v` versioned directories.** They select the highest version and stop, with
  no fallback when that image fails OS matching, so they cannot hold per-slot
  copies. Verified empirically; Flatcar reached the same conclusion.
- **`systemd-tmpfiles`.** No specifier identifies the booted device, partition
  or slot, and the main pass is ordered after the merge.
- **A systemd generator.** Generators run sandboxed with most of the filesystem
  read-only, and `/var` is not mounted when they run.
- **Reordering the sysext merge ahead of the `/etc` overlay.** Exhausts the
  filesystem stacking budget and breaks confexts outright.

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

Fedora CoreOS is the counter-example: it activates sysexts statically across
deployments, and its documentation records that after a major rebase the
extensions silently stop matching and are not loaded.

## Open questions

1. **Is the initrd per-slot or shared?** Decides whether a staged rollout is
   needed at all, and changes the estimates above.
2. **Whole-directory selection or a prepared set?** Both commit atomically. The
   former is simpler; the latter allows shared and slotted extensions to share a
   search path.
3. **Retention.** Nothing prunes a slot directory when an entry leaves the
   configuration. Reconciling during provisioning conflicts with keeping the
   previous slot's files for rollback. 0789's inventory gives this something to
   act on, but the policy is undecided.
4. **Default store path.** `/var/trident/extensions` against
   `/var/lib/trident/extensions`; the datastore already lives under the latter.
5. **Does a slotted extension need an extension-only update path?** 0789's
   Runtime Update path writes to the running slot, which is in tension with
   slot-scoped placement.
6. **Measured boot.** The selected image is outside the UKI's measurements, so
   identical measurements can yield different `/usr` contents.
7. **SELinux.** Extensions are already unsupported with SELinux enforcing, a
   precondition neither RFC resolves.

## Future possibilities

- **Native confext mounting.** On systemd 256 or later, the loader places the
  selected payload in a confext search path and `systemd-confext --mutable=yes`
  mounts it. The loader still performs selection, and payloads do not change
  provided they declare `ID=_any`. Azure Linux 4.0 ships systemd 258.4.
- **Content matching** on platforms with systemd v256 or later and a
  build-granular `VERSION_ID`, which needs no loader at all.
- **Initrd-phase sysext merging**, as Flatcar now does, for extensions that must
  be present before unit enumeration.
- Per-volume scoping for other Trident-managed state that is shared today.
