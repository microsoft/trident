# 0838 Slotted Extension Storage

- Date: 2026-10-08
- RFC PR: [microsoft/trident#838](https://github.com/microsoft/trident/pull/838)
- Issue: TBD

## Summary

Trident requires every extension destination to sit on an A/B volume when A/B is
configured. That is one way to make an extension follow the slot, and it costs a
partition pair per extension directory.

This adds a second. An extension marked `scope: slotted` is stored per volume
under a Trident-owned store, with a symlink at its normal destination.
Slot-scoping becomes a property of the extension rather than of the partition
table.

## Motivation and Goals

`validate_extension_images_locations` (`config/host/mod.rs:218`) rejects any
extension directory that does not resolve to a device with A/B capabilities,
raising `ExtensionImageNotOnABVolume`. The reasoning is sound: extensions on
shared storage change the running slot immediately and do not roll back.

The cost is that the only way to satisfy it is a dedicated A/B volume pair.
Trident's own reference configuration
(`tests/e2e_tests/trident_configurations/root-verity/trident-config.yaml`)
spends four partitions and 2.4 GB on it: `sysexts-a`/`sysexts-b` at 1 GB each
mounted at `/var/lib/extensions`, and `confexts-a`/`confexts-b` at 200 MB each
at `/var/lib/confexts`.

That price buys only slot-scoping, and it is charged in the currency hardest to
change:

- **Partitions are fixed at install.** The 1 GB is a guess made before anyone
  knows what the host will carry; growing it means repartitioning.
- **Some layouts cannot pay it.** Where the partition table is published, signed
  or attested, adding a pair is not a configuration change. A layout whose only
  A/B volume is a verity-sealed `/usr` has none to offer on any terms.
- **It is all-or-nothing per directory.** An extension that should follow the
  slot and one that should persist cannot share a location.

The check conflates *being slot-scoped*, the property that matters, with *being
on an A/B block device*, one implementation of it. Trident can provide the same
property with directories, on storage the host already has.

**Goals.** Slot-scoped extensions without a dedicated volume pair; rollback
restores the previous set; shared and slotted coexist in one directory; no new
partitions, no sealed-`/usr` change, no re-signing; existing configurations
unaffected.

**Non-goals.** Changing `systemd-sysext` matching semantics, which still decides
what merges. Removing the A/B volume route, which stays better where a pair is
affordable, since the filesystem provides the isolation and Trident does
nothing. Extensions on the ESP.

## Scope

Sysexts and confexts under `os.sysexts` and `os.confexts`, on clean install, A/B
update and rollback. Runtime update does not change slot and is unaffected.

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

Two optional fields. `path` keeps one meaning in both cases: where the extension
appears to systemd.

```yaml
os:
  # Root of the per-volume extension store.
  # Default /var/trident/sysexts. Must be on storage an A/B update does not
  # replace, and must not be a systemd-sysext search path.
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

| `scope` | Where the bytes live | What `path` is |
| --- | --- | --- |
| `shared` (default) | At `path`. | The image itself. Today's behaviour. |
| `slotted` | `<extensionStore>/{a,b}/{name}.raw` | A symlink to the booted slot's copy. |

`extensionStore` is configurable because slotting doubles storage, and large
extensions such as GPU drivers may warrant a dedicated volume. Confexts use a
sibling `confexts/` under the same root.

### The validation change

`validate_extension_images_locations` is relaxed, not removed. Its requirement
becomes *the destination is slot-scoped*, satisfied either way:

| Configuration | Rule |
| --- | --- |
| No `abUpdate` | No check, as today. |
| `abUpdate`, `scope: shared` | `path` must be on an A/B volume. Unchanged. |
| `abUpdate`, `scope: slotted` | `path` need not be; the store provides the scoping. |

This is the substantive behavioural change. Everything else follows from it.

### Validation

| Condition | Result |
| --- | --- |
| `scope` absent | `shared`. Identical to today. |
| `scope: slotted`, no `path` | Default destination, as today. |
| `scope: slotted`, no `abUpdate` | Reject, naming the absent configuration. |
| `extensionStore` inside a search path | Reject: both slots' copies would be discovered. |
| `extensionStore` on an A/B volume | Reject: it must outlive the slot it describes. |
| `extensionStore` set, no `slotted` entries | Accept, unused. |

### Backward compatibility

Both fields default, so existing Host Configurations parse and behave
identically and no field changes meaning. The relaxed check is strictly more
permissive, so anything valid today stays valid. The schema gains two optional
properties.

### Alternatives rejected

- **Sentinel `path`** (`/var/trident/sysexts/<slot>/x.raw`): leaks internal
  layout, invites a literal `a` or `b`.
- **Separate `os.slottedSysexts` list**: duplicates the type and its duplicate
  hash and path validation.
- **Infer from `extension-release`**: inside the image, so unavailable at static
  validation, and it removes the operator's choice to slot an `ID=_any`
  extension deliberately.

## Implementation

```
/var/trident/sysexts/
├── a/platform.raw
└── b/platform.raw

/var/lib/extensions/
├── platform.raw -> /var/trident/sysexts/a/platform.raw   # slotted
└── tooling.raw                                           # shared, unchanged
```

The store sits outside every search path, so only the symlink is discoverable
and exactly one copy is ever a candidate. Both slot directories persist across
an update, so rollback needs no re-download.

Symlinks are followed: `image_discover` opens each search path with
`chase_and_opendir(path, root, CHASE_PREFIX_ROOT, ..)` and stats entries with
`flags = 0` on a running system, with an upstream comment explicitly permitting
symlinks into the search path (systemd `src/shared/discover-image.c`).

### Servicing

The extensions subsystem already runs on clean install and A/B update via
`provision()`, documented as "migrate state from A-partition to B-partition (or
vice versa)". For a slotted entry it resolves the target volume with
`EngineContext::get_ab_update_volume()`, already used to select the per-slot
verity addon, writes into `<store>/<target>/`, and points the destination
symlink at it.

### Rollback

`RUNS_ON_ALL` is `CleanInstall | AbUpdate | RuntimeUpdate` (`engine/mod.rs:63`);
it excludes both rollback types despite the name. The extensions subsystem adds
`ManualRollbackAb` to its `runs_on` and repoints the symlinks at the previous
volume's directory. The files were never removed, so this is a symlink operation
rather than a re-download. `finalize_rollback` already locks `SUBSYSTEMS` for
the runtime path.

**Automatic rollback is not a servicing operation.** When the firmware falls
back on its own, Trident regains control only at `trident commit`, which is
`WantedBy=multi-user.target` and so runs after `systemd-sysext` merges at
`sysinit.target`.

The outcome is still safe, because the matching rules that motivate this RFC act
as an interlock. The stale symlink resolves to the other slot's extension, which
declares the other OS version, so systemd declines to merge it. The host comes
up with the extension **absent, not wrong**. `commit` then detects the rollback
through the existing `validate_boot` path, repoints the symlinks and runs
`systemd-sysext refresh`, which is the unit's own `ExecStart` and needs no
reboot.

Degraded for part of one boot, never serving content from the wrong OS version,
and self-closing. Where that window is unacceptable, a
[health check](../../Reference/Host-Configuration/API-Reference/Health.md)
asserting the extension is merged gates the update before it reaches this state.

## Relationship to RFC 0789

[0789](https://github.com/microsoft/trident/pull/789) carries extension images in
COSI files. The two are orthogonal: 0789 changes where the bytes come from, this
changes where they land and how they follow the slot. Either is useful alone,
and a bundled extension can be slotted.

Both touch `validate_extension_images_locations`, in opposite directions: 0789
tightens it into dynamic validation against the storage graph, this adds a
second way to satisfy it. Whichever lands first should leave the check expressed
as *is this destination slot-scoped*, so the other is an added arm rather than a
rewrite. 0789's rollback argument, that extensions revert because the slot's
filesystem reverts, is the A/B volume mechanism described here and holds
unchanged for `scope: shared`.

## Testing

- Clean install: file in the correct slot directory, symlink resolving, merged
  after boot.
- A/B update: the symlink repoints and **the merged content differs from the
  previous slot**. A presence-only assertion passes in the inert case and must
  not be used.
- Manual rollback: symlinks and merged content return to the previous slot.
- Automatic rollback: on the fallback boot the extension is absent rather than
  stale, and merges after `commit` without a reboot. Both halves asserted; the
  first distinguishes safe degradation from serving wrong content.
- Coexistence: a shared and a slotted extension in `/var/lib/extensions`, both
  merged, only the slotted one changing across an update.
- Validation: each reject row, plus a shared extension off an A/B volume still
  rejected.
- Compatibility: a configuration with no `scope` places byte-identically to
  today. A non-default `extensionStore` on a separate mount.

## Counter-arguments

- **"Just add the partitions."** Better where possible. This is for layouts that
  cannot, and for sizing decided after install rather than before.
- **"Put it in `/usr`."** Better still, and unavailable without re-sealing
  dm-verity and re-signing the UKI. Upstream also forbids extension storage
  under `/usr`: a `lowerdir=` that is a child of another fails with `-ELOOP`.
- **"Use the ESP."** Genuinely slot-scoped via `<uki>.efi.extra.d/`, but ESPs
  are 128 to 256 MiB against payloads in the tens to hundreds of megabytes, and
  `systemd-stub` places them in the initrd.
- **"Doubles storage."** It does. The A/B volume route doubles it too, with the
  allocation fixed at install.
- **"A symlink is weaker than a partition boundary."** Yes, though both are
  reachable by anything that can already write the extension image.

**Prior art.** systemd's versioned directories (`<name>.v/` holding
`<name>_<version>.raw`, highest wins) are the same shape: a directory of
candidates, one selected at discovery. They select by version rather than slot,
so they do not serve this case, but the pattern is systemd's own.

## Open questions

1. **Retention.** Nothing prunes a slot directory when an entry leaves the
   configuration. Should `provision()` reconcile, and does that conflict with
   keeping the previous slot's files for rollback?
2. **Default path.** `/var/trident/sysexts` against `/var/lib/trident/sysexts`;
   the datastore already lives under `/var/lib/trident/` (`constants.rs:30`).
3. **Should `commit` refresh unconditionally?** Refreshing only on a detected
   rollback is narrower, but an unconditional refresh is cheap and removes a
   branch.
4. **Host Status.** Should it report the slotted set per volume?
5. **SELinux.** Extensions are already unsupported with SELinux enforcing.
   Slotting does not change that, but Trident-created symlinks may need
   labelling, and 0789 proposes moving that check to every servicing type.

## Future possibilities

- Per-volume scoping for other Trident-managed state that is shared today.
- Changing an extension's scope between operations, which would otherwise strand
  a copy in the old location.
