# Trident installer

`installer` builds the `trident-installer` binary and optional RPM. It is an
autorun wrapper around Trident's gRPC API, not a partitioning engine.

## Local demo

```console
cargo run -p installer -- --demo
cargo run -p installer -- --demo failure
cargo run -p installer -- --demo no-images
cargo run -p installer -- --demo already-present
cargo run -p installer -- --demo disconnect
```

The demo never mounts media, inspects disks, contacts Trident, or changes machine
power state. Press `Q` to quit. Shell opens as the invoking user. Use `--plain`
for line-oriented output; without a terminal the demo prints its events and
exits when complete.

## Configuration

Read `/etc/trident/installer.toml` or the path supplied with `--config`.
`mode` is mandatory. All other fields are optional:

```toml
mode = "autorun"

[media]
cdromLabel = "TRIDENT_INSTALL"
cosiDirectory = "cosi/"
# Instead of cdromLabel, supply mountPath for already-mounted media.

[autorun]
# If omitted, use /etc/trident/config.yaml when that file exists.
# hostConfiguration = "installer/host-configuration.yaml"
reboot = true
```

Unknown fields and conflicting media selectors are errors. `interactive` is
reserved for later work, not an alias for autorun. COSI directory paths must
stay beneath the media root.

Media is mounted read-only at `/run/trident/installer-media`. Duplicate labels
are rejected. Optional `installer/installer.toml` on the mounted media can
override mode, image directory, and autorun options, but not bootstrap media
discovery. Relative HC paths are media-relative; absolute paths and `file://`
refer to the live filesystem. HTTP(S) HC sources are supported.

With an HC, use Install. Otherwise stream the first regular `.cosi` file in
the configured folder, sorted by filename, without recursing or following
image symlinks. Invalid HC or selected-image input is an error, not permission
to choose another operation or image.

An HC can use the installer-only image reference:

```yaml
image:
  url: installer://image
  sha384: ignored
```

The marker becomes the selected image's encoded `file://` URL before validation
and submission. Authored checksum settings are preserved. An explicit HC image
URL stays unchanged and needs no ISO image. Configure a checksum for production
images rather than copying the deliberately unverified example above.
HC disk paths are used as authored. The installer does not resolve legacy disk
placeholders or automatically choose a disk for an HC.

## Console and recovery

On the ISO, `--system-console` prefers the graphical virtual console for local
keyboard/monitor and BMC KVM, independent of `console=` order. An active serial
console is the fallback. The chosen getty is stopped; other active consoles
receive plain status once and identify the interactive console.

No ordinary source picker or confirmation precedes autorun. Missing sources
and failures show scrollable error details. Continue opens recovery: Shell,
Stream COSI URL, remote HC URL, or Shutdown. Shell `exit` returns to the
originating screen. Installation continues while a shell is open; automatic
reboot waits. The success screen offers Reboot and Shell.

Display filtering never removes daemon log records. All received responses are
also appended to the private `/var/log/trident-installer.log`. `V` toggles
detailed logs; PgUp/PgDn scroll. A missing final Completed response means unknown
outcome, not success. Another write requires a successful Trident status query
establishing that the daemon is idle.

Streaming checks COSI filesystem UUIDs before writing. A complete match opens
the already-present screen with a separately confirmed Force reinstall action.
It is an identity heuristic, not proof of image contents or OS health. Mounted
or read-only potential stream targets are never force-overwritten.
**HC installation intentionally has no repeat-install/Force guard:** the HC is
the operator's authority to overwrite its selected disks. Remove ISO media or
correct boot order to avoid repeated HC-driven installations.

`autorun.reboot` defaults to true. Successful installations requiring reboot
show a short countdown, then reboot; false leaves the success screen. Errors,
unknown outcomes, already-present images, and no-servicing results never
automatically reboot.

## ISO integration

Install `trident-installer`, then explicitly enable `trident-installer.service`
and `tridentd.socket` in the ISO recipe. Installing the RPM alone does not enable
the installer. Do not also enable `trident-install.service`. The daemon must be
built with `grpc-preview` for regular Install; repository RPM builds enable it.
Replace older installer startup units and autologin entrypoints rather than
running both installers on the same ISO.

The ISO volume label must match configuration. Image Customizer's existing
`CDROM` label requires an override or relabelling. With full-OS initramfs, keep
SELinux disabled and include USB storage/UAS, CD-ROM/SCSI, and USB host-controller
drivers. QEMU's IDE CD-ROM does not validate BMC virtual-media support.

## Validation

```console
cargo test -p installer
cargo test -p osutils terminal::tests
cargo clippy -p installer --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
```

Interactive disk-selection strategies and broader HC generation are follow-ups.
ISO boot, serial-terminal behaviour, RPM builds, and physical BMC virtual media
need separate integration validation.
