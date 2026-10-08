# 0838 Slotted system extensions

- Date: 2026-10-08
- RFC PR: [microsoft/trident#838](https://github.com/microsoft/trident/pull/838)
- Issue: TBD

## Summary

Add a `slotted` scope for sysexts/confexts. Trident stores them per A/B volume
under a configurable store (default `/var/trident/sysexts/{a,b}/`) and creates
the activating `/etc/extensions/<name>.raw` symlink during servicing.

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
set; no new partitions, no `/usr` change, no re-signing; existing configs
unaffected.

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
/var/trident/sysexts/{a,b}/platform.raw
/var/trident/confexts/{a,b}/
```

Both directories persist across the update, which is what makes rollback a
symlink flip rather than a re-download.

The extensions subsystem already runs on `CleanInstall` and `AbUpdate` via
`provision()`, documented as "migrate state from A-partition to B-partition (or
vice versa)".

1. **prepare** — download, verify SHA-384. Unchanged.
2. **provision** — resolve target volume via
   `EngineContext::get_ab_update_volume()` (already used for per-slot verity
   addons); place into `<store>/<target>/`.
3. **activate** — write `/etc/extensions/<name>.raw` →
   `<store>/<target>/<name>.raw`. `/etc` writes are already supported via
   `Subsystem::writable_etc_overlay()`.

The symlink supplies the `extension-release.NAME`-matching filename; the stored
file may be named freely. Same indirection distributions already use.

**Shared `/etc`.** The symlink is not slotted, so between servicing and reboot
the running OS points at an extension whose `VERSION_ID` does not match it. This
is inert: already-merged extensions stay merged, the mismatched one is not
merged.

**Rollback** repoints symlinks at the previous volume's directory. Inverse of
step 3.

**Ordering.** Symlinks are written before reboot, so they are correct before
`systemd-sysext.service` runs at `Before=sysinit.target`.

## Testing

- Clean install: file in correct volume dir, symlink correct, merged after boot.
- A/B update: symlink repoints and **merged content differs from the previous
  slot**. A presence-only assertion passes in the inert case and must not be
  used.
- Rollback: symlinks and merged content return to the previous slot.
- Validation: each reject row above.
- Compat: config with no `scope` places byte-identically to today.
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

1. **Does the rollback path invoke subsystem steps?** Largest unknown; gates the
   design.
2. **Should Host Status report slotted extensions per volume?** Derivable from
   `ab_active_volume` + store, but not inspectable without reading the FS.
3. **Retention.** Nothing prunes a slot directory when an entry is dropped from
   the config. Should `provision()` reconcile, and does that break rollback?
4. **SELinux.** Extensions are already documented as unsupported with SELinux
   enabled; a Trident-created `/etc` symlink may additionally need a label.
5. **Default path.** `/var/trident/sysexts` vs `/var/lib/trident/sysexts`; the
   datastore already lives under `/var/lib/trident/`.

## Future possibilities

- Per-volume scoping for other Trident-managed state that is shared today.
- Changing an extension's scope between servicing operations, which currently
  strands a copy in the old location.
