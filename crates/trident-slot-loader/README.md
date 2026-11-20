# Trident Slot Loader

Draft implementation of the boot selector in
[RFC 0838](https://github.com/microsoft/trident/pull/838). Installing its optional
RPM does not enable it or rebuild an initrd.

## Architecture

The binary has no public library API. Modules separate decisions from effects:

| Module | Responsibility |
| --- | --- |
| `main` | CLI dispatch and the absent-store fast path. |
| `config` | Typed environment parsing and initrd/host path derivation; no filesystem access. |
| `manifest`, `selection` | Validate input and construct an explicit link/mount plan; no runtime changes. |
| `symlinks` | Native sysext/confext links only. Never invokes a mount helper. |
| `mounts`, `release` | Read-only confext mounts, metadata compatibility and retained mount lifetime. |
| `outputs` | Validate and atomically publish Bootengine's lowerdir file. Never mounts images. |
| `loader` | Sequence the transaction and roll back its own resources on failure. |
| `recovery` | Pending-marker lifecycle and reboot gating, independent of manifest parsing. |
| `filesystem`, `tools` | Bounded/no-follow file operations and narrow mount/reboot adapters. |
| `diagnostics` | Leveled console records and monotonic phase timing. |

`systemd` confext mode creates a link-only plan. `overlay` creates confext
mounts plus sysext links. Only the latter mounts confexts inside the loader;
neither mode performs the native sysext merge.

## Input and configuration

The loader reads `acl.slot=a|b` and
`/sysroot/var/lib/aclslot/<slot>/slot-loader.json`:

```json
{
  "version": 1,
  "sysexts": [{"name": "runtime"}],
  "confexts": [{"name": "defaults"}]
}
```

Only listed, regular `<name>.raw` files under the selected slot are used.
Missing stores are a no-op; missing or invalid manifests in an existing store
fail. Unknown manifest fields are ignored; duplicate known keys or per-kind
names fail. Manifest order determines confext lowerdir precedence.

| Setting | Default | Behavior |
| --- | --- | --- |
| `LOADER_MODE` | `acl` | Select the slot from the kernel command line. |
| `TRIDEN_SLOT_STORE` | `/var/lib/aclslot` | Host-root store path; prefix `/sysroot` for initrd reads, not symlink targets. |
| `CONFEXT_MODE` | `overlay` | Prepare read-only mounts and a lowerdir list; `systemd` instead publishes native confext links. |
| `REBOOT_ON_UPDATE_LOADER_ERROR` | `true` | Recovery requests reboot only when the booted slot has a regular pending marker. |
| `TRIDENT_SLOT_LOADER_LOG_LEVEL` | `info` | Diagnostic verbosity; use `debug` for individual links and helper invocations. |

Recovery reads only its own setting and store paths. Broken activation
configuration or a malformed manifest cannot prevent it from checking a
pending update. Unknown modes, invalid Booleans and non-UTF8 mode/Boolean values fail
explicitly rather than selecting another backend.

Sysext links go into `/run/extensions`. The `systemd` confext backend uses
`/run/confexts`; it does not prepare Bootengine lowerdirs. Existing unrelated
entries are preserved; conflicting destinations fail. The same correct links
can be reused when the loader is invoked again.

The `overlay` backend invokes systemd 255's `systemd-dissect` to mount DDIs
read-only, checks confext release metadata against the booted OS's factory
`os-release`, and publishes `/run/trident-slot-loader/etc-lowerdirs` last.
Bootengine must consume that list before assembling writable `/etc`.
Mounts currently remain in the shared initrd namespace. Do not add
`PrivateMounts=` or sandboxing that implicitly hides them from Bootengine.

## Publication and failure boundaries

1. Validate the complete manifest and selected image paths.
2. Preflight native destinations and the lowerdir output before mounting.
3. Mount and validate all overlay confexts in manifest order.
4. Publish native links without replacing unrelated entries.
5. Atomically publish the lowerdir list, or remove a stale owned list for
   link-only activation. This commits the runtime handoff.
6. Remove and synchronize the pending marker.

Before handoff commit, errors and Rust unwinding trigger reverse-order cleanup.
Matching pre-existing links are not owned by the transaction. Replaced
destinations are left untouched; failed unmounts retain their mountpoints.
Cleanup failures are reported alongside the original error, not hidden.
Link creation is per-file, not an atomic multi-link operation: required service
ordering must keep consumers from running until the loader succeeds.

Manifest reads are limited to 1 MiB and release files to 64 KiB. Manifest
symlinks are rejected; opened inputs must be regular files, so FIFOs cannot block reads.
Confext lowerdirs and release files must resolve inside their mounted image.
Factory release metadata must resolve inside the booted OS, not the initrd.

On success, including an empty manifest, the loader removes and synchronizes
`slot-loader.pending`. The independent `--recover` path never parses the
manifest, changes boot entries or re-arms a failed slot.
Marker cleanup failure still fails the unit after runtime handoff commit.
SIGKILL, process abort and a hung kernel cannot run Rust cleanup.

## Observability

Records include `event`, phase, slot, backend, counts and relevant paths.
Every phase emits start/completion or failure with monotonic `elapsed_ms`.
Info logs distinguish preparation from native merge; debug logs add individual
link targets and helper exit statuses. Helper stderr streams directly to the
journal/console rather than accumulating in initrd memory.

Systemd priority prefixes preserve severity. Journald supplies boot ID, unit
and process identity; recovery additionally records the failed unit/result
provided by `OnFailure`. Inspect one boot with:

```text
journalctl -b -u trident-slot-loader.service -u trident-slot-loader-recovery.service -o short-monotonic
```

Initrd logs still require platform journal persistence to survive reboot.

## Integration boundary

The binary deliberately avoids the servicing engine, datastore and `osutils`
dependency graph. Its small private tool wrapper is an initrd-specific
exception to the usual shared subprocess-wrapper placement.

The following are not enabled or proven by this draft:

- Dracut inclusion of the binary, units, environment file, DDI helpers and
  their dependencies before normal UKI signing.
- Bootengine consumption of the lowerdir list, with `Requires=` and `After=`
  on the loader. Ordering alone must not permit root setup after loader failure.
- Staging of complete manifests, pending markers and slot-owned images.
- Native merge health checks and `systemd` backend enablement only where
  writable `/etc` semantics and consumer ordering are preserved.
- Private mount preparation/publication, platform signing/attestation policy,
  SELinux integration, crash/power-loss and real ACL boot validation.

The store and runtime directories require trusted root-only writers. Directory
checks reject symlinks but are not an adversarial concurrent-write sandbox.
Native systemd performs sysext/confext compatibility checks for link-only
activation. Loader success is preparation success, not proof that native merges
or the rest of the boot succeeded. The recovery hook does not cover Bootengine
failure after loader success, nor a hung kernel.

## Validation

`cargo test -p trident-slot-loader` exercises slot/manifest rejection,
preservation of unrelated links, overlay handoff, failure cleanup and recovery
gating with temporary files and mocked mount/reboot operations. Tests never
mount images or request a real reboot.
