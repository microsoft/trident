# Trident Linux Installer

`installer` builds the `trident-installer` binary and optional RPM. It is an
autorun wrapper around Trident's gRPC API through the shared `trident-client`
crate, not a partitioning engine.

## Local demo

```console
cargo run -p installer -- --demo
cargo run -p installer -- --demo failure
cargo run -p installer -- --demo no-images
cargo run -p installer -- --demo already-present
cargo run -p installer -- --demo disconnect
```

The demo never mounts media, inspects disks, contacts Trident, or changes machine
power state. Reboot, Shutdown and Shell are all simulated; no real shell or
command is launched. Press `Q` to quit. Use `--plain`
for line-oriented output; without a terminal the demo prints its events and
exits when complete.

## Configuration

Read `/etc/trident/installer.toml` or the path supplied with `--config`.
See the [Installer Configuration reference](../../docs/Reference/Installer-Configuration.md)
for all fields, defaults, and media-overlay rules.
`mode = "autorun"` is required and controls startup, not the servicing
operation: a Host Configuration selects Install; without one, the first media
COSI selects StreamDisk. All other fields are optional:

```toml
mode = "autorun"
serialMode = "logs"
serialVerbosity = "debug"

[media]
cosiDirectory = "cosi/"
# Optional: filesystemLabel = "CUSTOM_LABEL"
# Or use mountPath for already-mounted media.

[autorun]
# If omitted, use /etc/trident/config.yaml when that file exists.
# hostConfiguration = "installer/host-configuration.yaml"
reboot = true
force = false
```

Unknown fields and conflicting media selectors are errors. `interactive` is
reserved for later work, not an alias for autorun. COSI directory paths must
stay beneath the media root.
`serialMode` defaults to `logs`. `ui` is reserved and currently rejected with
an error rather than showing an unfinished serial TUI.
`serialVerbosity` defaults to `debug` and accepts `off`, `error`, `warn`,
`info`, `debug`, and `trace`.

`media.filesystemLabel` is a filesystem label, not a CD-ROM device name. When
omitted, the installer looks for either `TRD_INSTALL` (FAT-compatible, 11
characters) or `CDROM` (Image Customizer's ISO label), and rejects ambiguity
if both are present. Set it explicitly for other labels or when booting from
an optical ISO alongside a separate installer USB.
The older `cdromLabel` key is accepted as an alias; setting both is an error.
Media is mounted read-only with filesystem auto-detection at
`/run/trident/installer-media`. Media discovery waits up to 60 seconds for
USB/BMC block-device enumeration during autorun before reporting a missing
source; manually supplied recovery URLs do not wait. Other filesystem types
require corresponding kernel mount support on the live OS. Duplicate
labels are rejected. Optional
`installer/installer.toml` on the mounted media can
override mode, image directory, and autorun options, but not bootstrap media
discovery or `serialMode`. Relative HC paths are media-relative; absolute paths and `file://`
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

The UI uses the cyan Trident ASCII wordmark and the name Trident Linux
Installer. At 80x24 and larger the complete wordmark is shown; smaller terminals
use a compact text header to preserve usable controls and error details.

On the ISO, `--system-console` uses the graphical virtual console for local
keyboard/monitor and BMC KVM, independent of `console=` order. With
`serialMode = "logs"`, active serial consoles receive only formatted installer
and gRPC logs, never a TUI, even when no graphical console exists. Autorun
continues without an interactive console; missing inputs, repeat-install
protection and other cases needing operator input emit an error and stop.
Failures to open or write a serial console are logged and mirroring to that
console stops; they do not stop installation. On boots with
`trident.installer=1`, the RPM's getty drop-ins prevent serial login prompts
from mixing with the log stream and prevent tty1 getty from taking input.
Firmware, kernel and systemd output during boot is separate from the
installer's stream and may precede it; scrapers should match tagged log lines.

No ordinary source picker or confirmation precedes autorun. Missing sources
and failures keep recent logs visible in a red result frame; `D` opens the
scrollable full error details and Esc returns to the logs. Successful installs
keep their logs in a green result frame. Each result is also the last highlighted
log entry. Continue opens recovery: Shell,
Stream COSI URL, remote HC URL, or Shutdown. Shell `exit` returns to the
originating screen. The recovery shell is a root login Bash, so it reads
`/etc/profile` and the root profile (if present); on the installer ISO,
`/etc/profile` gives root a red prompt. Installation continues while a shell
is open; automatic reboot waits. The success screen offers Reboot and Shell.
Before each recovery attempt the baked configuration is reloaded, so shell
edits take effect. Invalid or missing configuration never supplies implicit
defaults for an installation.

Display filtering never removes daemon log records. All received responses are
also appended to the private `/var/log/trident-installer.log`. Installer events
and daemon records use `MM:SS [INST:LEVEL] message` and
`MM:SS [TRIDENT:LEVEL] message`, respectively. `serialVerbosity` controls the
serial stream independently of TUI verbosity; each line of a multiline message
gets its own `MM:SS [SOURCE:LEVEL]` prefix. The TUI defaults to Debug: errors red, warnings
amber, INFO pale neutral, DEBUG steel gray and TRACE slate gray; INST labels
are muted slate and TRIDENT labels seafoam. The wordmark remains cyan and
success is green. On the graphical Linux virtual terminal, the installer
programs its 16 ANSI palette slots to A1 RGB values, then restores the
original palette when opening a shell or leaving the UI. Workstation demo
terminals use the RGB values directly.
`V` opens a live verbosity picker (Off,
Error, Warn, Info, Debug, Trace) beside the operation; this only filters the
display, not serial or stored diagnostics. The private diagnostic file retains
all levels even when serialVerbosity filters them. PgUp/PgDn scroll.
TRACE output can expose low-level inputs; restrict access to BMC serial captures.
A missing final Completed response means unknown
outcome, not success. Another write requires a successful Trident status query
establishing that the daemon is idle.

Streaming checks COSI filesystem UUIDs before writing. A complete match opens
the already-present screen with a separately confirmed Force reinstall action.
`autorun.force` defaults to false. Setting it to true skips the matching-UUID
check on automatic COSI streams, without weakening the mounted/read-only target
checks. It does not change HC installs or manually supplied recovery URLs.
**With force enabled, an ISO left in the boot path can reimage the machine on
every boot.** Remove or deprioritize the media after installation.
Mounted or read-only potential stream targets are never force-overwritten.
If the installer USB itself is the smallest disk fitting the COSI, streaming
stops rather than overwriting it; use a USB smaller than the COSI's original
disk size or an HC selecting an explicit target. The current StreamDisk RPC
cannot exclude a specific disk.
**HC installation intentionally has no repeat-install/Force guard:** the HC is
the operator's authority to overwrite its selected disks. Remove ISO media or
correct boot order to avoid repeated HC-driven installations.

`autorun.reboot` defaults to true. Successful installations requiring reboot
show a short countdown, then reboot; false leaves the success screen. Errors,
unknown outcomes, already-present images, and no-servicing results never
automatically reboot.

## ISO integration

On a full-OS installer ISO, install `trident-installer`, add
`trident.installer=1` to its **ISO kernel command line**, and provide a COSI
under `cosi/` or a Host Configuration. The RPM pulls in Trident and common
installer tools, enables its service, starts the gRPC socket through the
service dependency, and suppresses tty1 and serial gettys on marked boots.
Without that flag the service skips startup and normal gettys are unchanged,
including on an installed OS that happens to have the RPM.

Do not also enable `trident-install.service` or older autologin entrypoints.
The daemon must be built with `grpc-preview` for regular Install; repository
RPM builds enable it. For Image Customizer ISOs, `CDROM` is automatically
recognized; for GPT USB media, `TRD_INSTALL` is automatically recognized.
Specify `media.filesystemLabel` in the bootstrap TOML only to override those
labels or disambiguate multiple matching devices. With full-OS initramfs,
keep SELinux disabled and include USB storage/UAS, CD-ROM/SCSI, and USB
host-controller drivers. QEMU's IDE CD-ROM does not validate BMC virtual
media support.

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
