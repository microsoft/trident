---
sidebar_position: 7
title: Installer Configuration
---

# Installer Configuration (TOML)

`trident-installer` reads `/etc/trident/installer.toml` by default. Pass
`--config PATH` to use another file. This TOML file controls the **installer**;
the [Host Configuration](./Host-Configuration/Host-Configuration.md) is a
separate YAML document describing what Trident installs. A live installation
requires a readable TOML file. `--demo` does not read it.

The packaged `/etc/trident/installer.toml` explicitly sets:

```toml
mode = "autorun"
serialMode = "logs"
serialVerbosity = "debug"

[media]
filesystemLabel = "TRD_INSTALL"
cosiDirectory = "cosi/"

[autorun]
reboot = true
force = false
```

There is **no implicit default for `mode`** in a TOML file: omitting it is an
error, even though the packaged file sets it to `"autorun"`. All other fields
may be omitted; unknown fields and invalid values are errors.

## Selecting Install or StreamDisk

`mode = "autorun"` means **start without an interactive source picker**. It
does not choose the Trident operation; `mode = "install"` and `mode = "stream"`
are not valid TOML settings. After looking for media and applying any
[media-local overrides](#optional-configuration-on-the-media), autorun makes
this choice:

| Priority | Condition | Operation |
| --- | --- | --- |
| 1 | `autorun.hostConfiguration` is set | **Install** using that Host Configuration, even if a COSI is present. |
| 2 | Otherwise, `/etc/trident/config.yaml` exists in the live OS | **Install** using that file, even if a COSI is present. |
| 3 | Otherwise, media contains a `.cosi` file | **StreamDisk** using the first regular COSI in `media.cosiDirectory`, sorted by filename. |
| 4 | Neither source is available | Show recovery actions on the graphical console, or report an error without one. |

For example, to **Install** from a Host Configuration placed on an
Image Customizer ISO:

```toml
# /etc/trident/installer.toml in the live installer OS
mode = "autorun"

[media]
filesystemLabel = "CDROM"

[autorun]
hostConfiguration = "installer/host.yaml"
reboot = false
```

Put `installer/host.yaml` on the ISO filesystem (not only in the live OS).
If its `image.url` is `installer://image`, also put a COSI in `cosi/`; the
marker resolves to the first discovered COSI, but the operation remains
**Install**, with disk layout taken from the Host Configuration. An explicit
image URL in the Host Configuration does not require a media COSI. See the
[Host Configuration reference](./Host-Configuration/Host-Configuration.md)
for its YAML schema. To **StreamDisk** instead, leave
`autorun.hostConfiguration` unset *and* ensure
`/etc/trident/config.yaml` is absent from the live OS. A specified but
invalid Host Configuration fails rather than falling back to streaming.

## Fields

The Default column gives the effective value when a key is omitted, not
necessarily the value written in the packaged file.

| Key | Default | Description |
| --- | --- | --- |
| `mode` | **None; required** | Set `"autorun"` (as the packaged file does). This is a startup mode, not the Install/StreamDisk selector. `"interactive"` is reserved and rejected. |
| `serialMode` | `"logs"` | Sends plain installer and Trident log records to active serial consoles, without a serial TUI. `"ui"` is reserved and rejected. |
| `serialVerbosity` | `"debug"` | Serial log filter: `"off"`, `"error"`, `"warn"`, `"info"`, `"debug"`, or `"trace"`. Independent of the on-screen log filter, which starts at Debug. |
| `media.filesystemLabel` | `"TRD_INSTALL"` | Exact filesystem label to find on optical media or a USB partition. `TRD_INSTALL` fits FAT's 11-character label limit; Image Customizer ISOs use `CDROM` instead. |
| `media.cdromLabel` | — | Legacy alias for `filesystemLabel`. Do not specify both names. |
| `media.mountPath` | Unset | Absolute path to an **already mounted** filesystem instead of label discovery. Cannot be combined with `filesystemLabel`; this path is not mounted or made read-only by the installer. |
| `media.cosiDirectory` | `"cosi"` | Nonempty directory relative to the media root. Absolute paths and `..` are rejected. Only regular `.cosi` files directly inside it are considered, in filename order. |
| `autorun.hostConfiguration` | Unset | Host Configuration source. When omitted, `/etc/trident/config.yaml` is used **if it exists**; otherwise autorun looks for a COSI. |
| `autorun.reboot` | `true` | Reboot after successful servicing **only if** Trident reports that work was performed and a reboot is required. |
| `autorun.force` | `false` | On automatic COSI streaming, bypass the matching-filesystem-UUID repeat-install guard. Does not affect Host Configuration installs, manual recovery sources, or the mounted/read-only disk safety check. |

`filesystemLabel` discovery waits up to 60 seconds during autorun for late
USB/BMC device enumeration. It fails if multiple devices have the label. The
selected filesystem is mounted read-only at `/run/trident/installer-media`;
the live OS must support its filesystem. A manually selected remote recovery
source does not wait for media.

### Host Configuration sources

`autorun.hostConfiguration` accepts:

| Form | Example | Resolution |
| --- | --- | --- |
| Media-relative path | `"installer/host.yaml"` | Beneath the mounted media root; cannot escape it. |
| Absolute path | `"/etc/trident/config.yaml"` | A file in the live installer OS. |
| File URL | `"file:///etc/trident/config.yaml"` | A file in the live installer OS. |
| HTTP(S) URL | `"https://example.com/host.yaml"` | Fetched over the network. |

Host Configuration input must be valid YAML and is limited to 2 MiB. A Host
Configuration image URL of `installer://image` fails if no COSI is available.
Authored disk paths and image checksums remain unchanged. Even with an
explicit Host Configuration source, autorun checks for media first (up to
the 60-second label-discovery wait) so it can resolve `installer://image`.

Without a Host Configuration, the first regular COSI in `cosiDirectory` is
streamed. If neither source is available, the graphical console offers
recovery; a headless run reports an error instead. Even with
`autorun.force = true`, streaming refuses a mounted or read-only disk that
Trident could select as its smallest fitting target, such as a large
installer USB.

## Optional configuration on the media

After finding or mounting media, the installer merges
`installer/installer.toml` from its root over the bootstrap configuration.
For example, this media-local file selects **Install** without changing the
bootstrap file baked into the live OS:

```toml
# /installer/installer.toml on the media
[autorun]
hostConfiguration = "installer/host.yaml"
reboot = false
```

The bootstrap `media.filesystemLabel` must already match the media for this
overlay to be found. Place the referenced `installer/host.yaml` on the same
media.

The media file can change `serialVerbosity`, `media.cosiDirectory`, `mode`,
and `autorun` options. It cannot change `serialMode`,
`media.filesystemLabel` (including `cdromLabel`), or `media.mountPath`:
those settings are needed to find the media in the first place. Other
fields still undergo the same validation; setting `mode = "interactive"`
remains unsupported.
