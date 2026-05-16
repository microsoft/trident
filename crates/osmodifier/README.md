# osmodifier

Native Rust port of the OS modifier functionality from
[azure-linux-image-tools](https://github.com/microsoft/azure-linux-image-tools).

Trident calls osmodifier functions directly as a library crate instead of
serializing config to YAML, writing a temp file, and exec'ing the Go binary.

## Port Origin

The initial port was made on **2026-05-11** (commit `ba55580`) from the
azure-linux-image-tools repository. The Go code spans three packages under
`toolkit/tools/`:

| Go package | Purpose |
|------------|---------|
| `osmodifier/` | CLI entry point |
| `osmodifierapi/` | Configuration types and validation |
| `pkg/osmodifierlib/` | Core modification logic |
| `pkg/imagecustomizerlib/` | Shared helpers (users, hostname, services, modules) |

## File Mapping

Each Rust source file and the Go file(s) it was ported from:

| Rust file | Go source(s) | Go commit | Date |
|-----------|--------------|-----------|------|
| `lib.rs` | `pkg/osmodifierlib/osmodifier.go`, `pkg/osmodifierlib/modifierutils.go` | `f4de1a0` | 2026-03-17 |
| `config.rs` | `osmodifierapi/os.go`, `osmodifierapi/overlay.go`, `osmodifierapi/verity.go`, `osmodifierapi/identifiedpartition.go` | `8bd4ef3` | 2025-09-02 |
| `users.rs` | `pkg/imagecustomizerlib/customizeusers.go` | `8bd4ef3` | 2025-09-02 |
| `hostname.rs` | `pkg/imagecustomizerlib/customizehostname.go` | `8bd4ef3` | 2025-09-02 |
| `modules.rs` | `pkg/imagecustomizerlib/kernelmoduleutils.go` | `8bd4ef3` | 2025-09-02 |
| `services.rs` | `pkg/imagecustomizerlib/customizeservices.go` | `dc90945` | 2026-03-31 |
| `selinux.rs` | `pkg/osmodifierlib/modifierutils.go` (SELinux functions) | `f4de1a0` | 2026-03-17 |
| `default_grub.rs` | `pkg/osmodifierlib/modifydefaultgrub.go` | `f4de1a0` | 2026-03-17 |
| `grub_cfg.rs` | `pkg/osmodifierlib/modifydefaultgrub.go`, `pkg/osmodifierlib/modifierutils.go` | `f4de1a0` | 2026-03-17 |

All Go paths are relative to `toolkit/tools/` in the azure-linux-image-tools
repository. The Go commit column is the latest commit touching that file at the
time of the port.

## Key Differences from the Go Implementation

### Library instead of binary

The Go osmodifier is a standalone CLI binary invoked via `exec`. The Rust
version is a library crate exposing three public functions:

```rust
osmodifier::modify_os(&ctx, &config)?;        // replaces: osmodifier --config-file
osmodifier::modify_boot(&ctx, &boot_config)?;  // replaces: osmodifier --config-file (boot subset)
osmodifier::update_default_grub(&ctx)?;         // replaces: osmodifier --update-grub
```

**Reasoning:** Eliminates YAML serialization round-trips, temp file I/O, and
process spawning overhead. Errors propagate as native Rust `Result` types
instead of being parsed from stderr.

### No chroot / safechroot

The Go code uses `safechroot` to enter a chroot environment before making
modifications. The Rust version operates on a mounted root directory via
`OsModifierContext`, prefixing all paths with the root directory.

**Reasoning:** Trident already manages the chroot lifecycle at a higher level.
Duplicating chroot enter/exit in osmodifier would conflict with the outer
chroot management. Path-prefixing achieves the same isolation without the
complexity.

### Inlined imagecustomizerlib logic

The Go osmodifier delegates user, hostname, service, and module management to
`imagecustomizerlib`, a shared library also used by the image customizer tool.
The Rust port inlines this logic into dedicated modules (`users.rs`,
`hostname.rs`, `services.rs`, `modules.rs`).

**Reasoning:** Trident only needs the osmodifier subset of imagecustomizerlib.
Porting the full shared library would pull in unnecessary dependencies. Inlining
keeps the crate self-contained and avoids coupling to Go-side refactors in the
shared library.

### Secure password handling

The Go code sets passwords via `useradd -p <hash>`, which exposes the password
hash in `/proc/<pid>/cmdline`. The Rust version uses `chpasswd -e` with the
hash passed via stdin.

**Reasoning:** Defense in depth. Any process on the system can read
`/proc/cmdline`, making the hash visible during user creation. Passing it via
stdin keeps the hash out of the process argument list.

### Atomic file writes

The Rust code uses `tempfile::NamedTempFile::persist()` for all writes to
sensitive files (`/etc/shadow`, `/etc/passwd`). The Go code writes directly.

**Reasoning:** Atomic rename prevents partial writes from corrupting critical
auth files if the process is interrupted mid-write.

### Startup command validation

The Rust code validates that startup commands do not contain colons or newlines
before writing to `/etc/passwd`. The Go code does not perform this validation.

**Reasoning:** `/etc/passwd` is colon-delimited and newline-separated. A
malicious or malformed startup command containing these characters could corrupt
the passwd file or inject additional entries.

### Split boot configuration API

The Go binary handles OS and boot modifications in a single `--config-file`
invocation. The Rust version splits this into `modify_os()` and `modify_boot()`
with separate config types (`OSModifierConfig` and `BootConfig`).

**Reasoning:** OS modifications (users, hostname, services) and boot
modifications (SELinux, overlays, verity) happen at different stages of the
Trident image build pipeline. Separating them avoids passing unused
configuration and makes the call sites clearer.

### System tool access via Dependency enum

External tool invocations use the trident `osutils::Dependency` enum instead
of calling `std::process::Command` directly. This provides consistent binary
resolution (via `which`), structured error reporting, and a centralized
inventory of runtime dependencies.

| Dependency variant | Used in |
|--------------------|---------|
| `Systemctl` | `services.rs` — enable/disable services |
| `Grub2Mkconfig` | `grub_cfg.rs` — regenerate GRUB config |
| `Chroot` | `users.rs` — run tools inside a mounted root |
| `Id` | `users.rs` — check if a user exists |
| `Useradd` | `users.rs` — create new users |
| `Usermod` | `users.rs` — modify groups |
| `Chown` | `users.rs` — set file ownership |

Two tools still use `std::process::Command` directly because the Dependency
`Command` wrapper does not yet support stdin piping:

- **`openssl passwd`** (`hash_password`) — reads plaintext from stdin
- **`chpasswd -e`** (`set_password_via_chpasswd`) — reads `user:hash` from stdin

## Keeping the Port in Sync

When the Go osmodifier code changes upstream, compare the diff against the
corresponding Rust module using the file mapping table above. Pay special
attention to:

- New fields added to config structs in `osmodifierapi/`
- New modification steps in `modifierutils.go`
- Changes to GRUB parsing logic in `modifydefaultgrub.go`
- Changes to user/service/module handling in `imagecustomizerlib/`
