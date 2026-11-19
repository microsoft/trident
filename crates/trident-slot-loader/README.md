# Trident Slot Loader

Draft implementation of the boot selector in
[RFC 0838](https://github.com/microsoft/trident/pull/838). Installing its optional
RPM does not enable it or rebuild an initrd.

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

On success, including an empty manifest, the loader removes and synchronizes
`slot-loader.pending`. The independent `--recover` path never parses the
manifest, changes boot entries or re-arms a failed slot.

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
