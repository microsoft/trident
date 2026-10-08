# 0838 Slotted system extensions

- Date: 2026-10-08
- RFC PR: [microsoft/trident#838](https://github.com/microsoft/trident/pull/838)
- Issue: TBD

## Summary

Add a `slotted` scope for sysexts/confexts. Trident stores them per A/B volume
under a configurable store (default `/var/trident/sysexts/{a,b}/`) and activates
the correct slot at boot, before `systemd-sysext` merges.

## Motivation

`systemd-sysext(8)` merges an extension only if its `extension-release` matches
the running OS: `ID=` must match unless `_any`, then `SYSEXT_LEVEL=` if defined,
else `VERSION_ID=`.

All three directories Trident accepts today (`/etc/extensions`,
`/var/lib/extensions`, `/.extra/sysext`) are outside the partitions an A/B
update replaces. So an OS-pinned extension silently stops merging after the
first update: no error, no failed unit, the content is just gone.

Azure Container Linux hits this. Its Azure platform extension (guest agent,
Hyper-V daemons, chrony, `oras`) is `VERSION_ID`-pinned on a non-swapped
partition. Losing `oras` also breaks the distro's extension download path.

The shape that works already exists in the same image: the container runtime
ships as an extension inside `/usr`, so extension and `os-release` swap
together. This proposal gives that property to anyone who cannot re-seal
dm-verity and re-sign the UKI.

Goals: correct extension version per booted slot; rollback restores the previous
set without a Trident servicing operation; no new partitions, no `/usr` change,
no re-signing; existing configs unaffected.

Non-goals: changing systemd matching semantics; ESP-based extensions.

## Public API Design

### Today

```rust
pub struct Extension {
    pub url: Url,
    pub sha384: Sha384Hash,
    pub path: Option<PathBuf>,  // absolute, allow-listed dir, *.raw
}
```

`path` conflates *where the file is stored* with *what scope it is active in*.
Slotting separates them: storage becomes Trident-owned and derived from the
target volume.

### Proposed

```rust
pub struct Extension {
    pub url: Url,
    pub sha384: Sha384Hash,

    /// Absolute path in the target OS. Only valid when `scope` is `shared`.
    pub path: Option<PathBuf>,

    #[serde(default)]
    pub scope: ExtensionScope,
}

pub enum ExtensionScope {
    /// Stored at `path`, active on both volumes. For `ID=_any` extensions.
    #[default]
    Shared,
    /// Stored per-volume in the extension store, active only on the volume it
    /// was installed to. For OS-version-pinned extensions.
    Slotted,
}
```

Plus one optional field on `os`:

```rust
/// Root of the per-volume extension store. Default `/var/trident/sysexts`.
/// Must be on storage not replaced by an A/B update, and must not be a
/// systemd-sysext search path.
pub extension_store: Option<PathBuf>,
```

Configurable because slotting doubles storage, and large extensions (GPU
drivers, ML runtimes) may need a dedicated volume. Confexts use a sibling
`confexts/` under the same root.

```yaml
os:
  extensionStore: /mnt/bigdisk/trident/sysexts   # optional
  sysexts:
    - url: https://example.com/tooling.raw       # unchanged, still default
      sha384: <hash>
    - url: https://example.com/platform.raw
      sha384: <hash>
      scope: slotted
```

### Validation

| Condition | Result |
|---|---|
| `scope` absent | `shared`. Identical to today. |
| `scope: shared` + `path` | Existing allow-list check. |
| `scope: slotted` + `path` | Reject. Path is Trident-owned; accepting it would silently ignore it. |
| `scope: slotted`, no `path` | Filename from `url` basename, as today's default. |
| `scope: slotted`, no A/B volumes | Reject at validation, not at provision. |
| `extensionStore` set, no `slotted` entries | Accept, unused. |
| `extensionStore` inside a sysext search path | Reject: both copies would be discovered. |

### Backward compatibility

Both fields are `#[serde(default)]`. Existing configs parse and behave
identically. Schema gains two optional properties.

### Alternatives rejected

- **Sentinel `path`** (`/var/trident/sysexts/<slot>/x.raw`): leaks internal
  layout, invites a literal `a`/`b`.
- **Separate `os.slottedSysexts` list**: duplicates the type and its
  duplicate-hash/path validation.
- **Infer from `extension-release`**: metadata is inside the image, unavailable
  at static validation, and removes the operator's choice to slot an `_any`
  extension deliberately.

## Implementation

```
/var/trident/sysexts/
├── a/platform.raw
├── b/platform.raw
└── current -> a        # flipped at boot
```

Both slot directories persist across an update, so a rollback needs no
re-download. Confexts use a sibling `confexts/` under the same root.

### Activation is resolved at boot, not at servicing

`/etc` is shared between volumes, so an activating symlink written during
servicing would still point at the new slot after a rollback. This cannot be
corrected by an inverse servicing step, because **the A/B rollback path does not
invoke subsystems**:

- Automatic rollback is firmware-driven. `rollback::validate_boot` only observes
  which root actually booted and reconciles Host Status (`spec = spec_old`).
- `manual_rollback::stage_rollback` touches only the pcrlock policy for
  `ManualRollbackAbStaged`. Only the *runtime* path calls
  `runtime_update::rollback(&mut subsystems, ..)`.

Activation must therefore key off what booted, not what was staged:

1. `/etc/extensions/<name>.raw` → `<store>/current/<name>.raw`. Written once at
   clean install, never rewritten.
2. `<store>/current` → `a` | `b`, flipped at boot by a Trident unit from the
   booted volume.

One atomic symlink flip activates the whole set, and it is correct after a
rollback Trident never participated in.

### Servicing flow

The extensions subsystem already runs on `CleanInstall` and `AbUpdate` via
`provision()`, documented as "migrate state from A-partition to B-partition (or
vice versa)".

1. **prepare** — download, verify SHA-384. Unchanged.
2. **provision** — resolve the target volume via
   `EngineContext::get_ab_update_volume()` (already used for per-slot verity
   addons); place into `<store>/<target>/`.
3. **configure**, clean install only — write the `/etc/extensions` symlinks.
   `/etc` writes are already supported via `Subsystem::writable_etc_overlay()`.

The symlink supplies the `extension-release.NAME`-matching filename; stored
files may be named freely.

### Boot unit

```
After=local-fs.target        # /var mounted, store readable
Before=systemd-sysext.service
```

`systemd-sysext.service` is `After=local-fs.target Before=sysinit.target`, so
this window exists. It is also gated on
`ConditionDirectoryNotEmpty=|/etc/extensions`, which the install-time symlinks
satisfy.

This cannot be `trident.service`, which is `WantedBy=multi-user.target` and runs
long after the merge.

Slot discovery: compare the booted `/usr` (or root) PARTUUID against the volume
mapping Trident recorded at install. Not the datastore, which is larger
machinery than this needs. See Open Questions.

## Testing

- Clean install: file in the correct slot dir, `current` and `/etc/extensions`
  symlinks correct, merged after boot.
- A/B update: `current` flips and **merged content differs from the previous
  slot**. A presence-only assertion passes in the inert case and must not be
  used.
- Rollback, both automatic and manual: `current` returns to the previous slot
  and the previous content is merged, **with no Trident servicing operation in
  between**. This is the case the design exists for.
- Boot ordering: the flip lands before `systemd-sysext.service` merges, asserted
  on first boot after an update rather than on a steady-state boot.
- Validation: each reject row above.
- Compat: a config with no `scope` places byte-identically to today.
- Non-default `extensionStore` on a separate mount.

## Counter-arguments

- **"Put it in `/usr`."** Strictly better where possible, but requires re-sealing
  verity and re-signing the UKI. This targets that gap.
- **"Use the ESP."** `<uki>.efi.extra.d/*.sysext.raw` is genuinely slot-scoped,
  but ESPs are 128-256 MiB against payloads in the tens to hundreds of MB, and
  `systemd-stub` places them in the initrd, not the running system.
- **"Add A/B extension partitions."** Works, but layout changes are breaking
  after the first A/B image ships. Directories achieve the same isolation.
- **"Doubles storage."** Yes. Mitigated by `extensionStore`.

## Open questions

1. **Slot discovery in the boot unit.** Comparing the booted PARTUUID against a
   small install-time mapping file is proposed, but the mapping must survive the
   update that rewrites it. Reading the datastore this early is the alternative.
2. **Does the flip need to be a Trident binary at all?** The operation is one
   `readlink` and one `symlink`. A generator or a small shipped script may be a
   better fit than adding a Trident invocation to early boot.
3. **Retention.** Nothing prunes a slot directory when an entry is dropped from
   the config. Should `provision()` reconcile, and does that break the rollback
   guarantee that the previous slot's files are still present?
4. **SELinux.** Extensions are already documented as unsupported with SELinux
   enabled; Trident-created symlinks in `/etc` may additionally need a label.
5. **Default path.** `/var/trident/sysexts` vs `/var/lib/trident/sysexts`; the
   datastore already lives under `/var/lib/trident/`.
6. **Host Status.** Should it report slotted extensions per volume? Derivable
   from `ab_active_volume` plus the store, but not otherwise inspectable.

## Future possibilities

- Per-volume scoping for other Trident-managed state that is shared today.
- Changing an extension's scope between servicing operations, which currently
  strands a copy in the old location.
