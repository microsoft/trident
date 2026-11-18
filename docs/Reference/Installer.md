---
sidebar_position: 7
title: Trident Installer
---

# Trident Installer

Install the optional `trident-installer` RPM on a full-OS installer ISO and add
`trident.installer=1` to the **ISO boot kernel command line**. The packaged
systemd service starts only when that argument is present; installing the RPM
alone does not start an installation. See
[Installer Configuration](./Installer-Configuration.md#building-an-installer-iso)
for an ISO recipe.

A [Host Configuration](./Host-Configuration/Host-Configuration.md) (HC) is YAML
describing the OS image, disk layout, and settings for Trident's **Install**
operation. A [Composable OS Image](./Composable-OS-Image.md) (COSI) contains
the image data that **StreamDisk** writes to a disk. **Autorun** selects one
of these operations from the available sources and starts without an initial
confirmation or source picker. The terminal UI shows progress and offers
recovery after a failure or interruption. `mode = "interactive"` is reserved
and currently rejected.

## Choosing the installation source

```mermaid
flowchart TD
  START(("START: installer boots")) --> CONFIG["Read installer TOML; require autorun"]
  CONFIG -->|Invalid| ERROR["ERROR"]
  CONFIG --> SELF{"Complete live-OS or HTTP(S) HC without media-backed inputs?"}
  SELF -->|Yes| INSTALL["Install with that HC; skip media overrides"]
  SELF -->|No| MEDIA["Find installer media; wait up to 60 seconds during autorun"]
  MEDIA --> OVERLAY["Apply media-local installer.toml, if present"]
  OVERLAY --> HC{"Configured HC or default config.yaml file or symlink?"}
  HC -->|Yes| READ["Read HC; resolve installer://image to first media COSI if used"]
  READ --> INSTALL
  HC -->|No| COSI{"Media contains a COSI?"}
  COSI -->|Yes| STREAM["StreamDisk: first regular COSI by filename"]
  COSI -->|No| ERROR
  STREAM --> PREFLIGHT["Check metadata hash and eligible disk safety"]
  PREFLIGHT -->|Failed| ERROR
  INSTALL --> TRIDENT["Send operation to Trident over gRPC"]
  PREFLIGHT --> TRIDENT
  TRIDENT -->|Completed successfully| SUCCESS["SUCCESS"]
  TRIDENT -->|Failure or unknown outcome| ERROR
  classDef startPoint fill:#07545b,stroke:#77e6db,color:#f0fffc,stroke-width:3px
  classDef success fill:#123c2a,stroke:#3fb950,color:#e7ffed,stroke-width:2px
  classDef error fill:#52212a,stroke:#f85149,color:#ffe8e5,stroke-width:2px
  class START startPoint
  class SUCCESS success
  class ERROR error
```

`installer://image` is a reserved HC image URL meaning "use the first COSI
found on installer media," not a filename. Media is also needed for a
media-relative HC, a dangling HC symlink into the media, or another file-backed
HC input on the media (such as an additional file, script, extension, or
recovery key). A self-contained HC takes
precedence over media settings even if the media is already present. An invalid
HC fails; it does not fall back to StreamDisk.

StreamDisk preflight reads the COSI's required disk size and metadata hash. It
refuses a mounted or read-only disk that Trident could select as the smallest
one that fits, then passes the hash to Trident so its fetch uses the checked
metadata. See [Installer Configuration](./Installer-Configuration.md) for
accepted paths, filesystem labels, and overrides.

:::warning Reinstall on every installer boot

Autorun does not ask for confirmation or check whether the target OS is already
installed. Remove or deprioritize the installer media after installation;
leaving it first in boot order can reimage the machine on every boot.

:::

## Screens

The same screen has the same color in both diagrams: blue **PROGRESS**, amber
**MENU**, red **ERROR**, purple **RECOVERY**, and green **SUCCESS**. Teal circles
mark entry points; teal rectangles are URL-entry screens.

```mermaid
flowchart TD
  START(("START: autorun")) --> P["PROGRESS: activity and logs"]
  P -->|Ctrl-C or Esc| M["MENU: Return to installation; Shell"]
  M -->|Return or Esc| P
  P -->|Completed successfully| S["SUCCESS: Reboot; Shell"]
  M -->|Completed successfully| S
  P -->|Failure or unknown outcome| E["ERROR: logs; Continue; Shell"]
  M -->|Failure or unknown outcome| E
  E -->|D| ED["Full error details"]
  ED -->|D or Esc| E
  E -->|Continue, Esc, or Ctrl-C| R["RECOVERY"]
  S -->|Ctrl-C| R
  classDef startPoint fill:#07545b,stroke:#77e6db,color:#f0fffc,stroke-width:3px
  classDef progress fill:#153858,stroke:#58a6ff,color:#f0f6fc,stroke-width:2px
  classDef menu fill:#493416,stroke:#d29922,color:#fff3dc,stroke-width:2px
  classDef success fill:#123c2a,stroke:#3fb950,color:#e7ffed,stroke-width:2px
  classDef error fill:#52212a,stroke:#f85149,color:#ffe8e5,stroke-width:2px
  classDef recovery fill:#342450,stroke:#b392f0,color:#f4ebff,stroke-width:2px
  class START startPoint
  class P progress
  class M menu
  class S success
  class E,ED error
  class R recovery
```

```mermaid
flowchart TD
  START(("START: enter recovery")) --> R["RECOVERY: Shell; Stream COSI URL; Install HC URL; optional Shutdown"]
  R -->|Stream COSI URL| U1["STREAM URL entry"]
  R -->|Install HC URL| U2["HOST CONFIG URL entry"]
  U1 -->|Esc| R
  U2 -->|Esc| R
  U1 -->|Invalid URL| E["ERROR"]
  U2 -->|Invalid URL| E
  U1 -->|Enter valid URL| P["PROGRESS: new attempt"]
  U2 -->|Enter valid URL| P
  P -->|Success| S["SUCCESS"]
  P -->|Failure or unknown outcome| E
  R -->|Shutdown when safe| OFF["Power off"]
  S -->|Reboot when safe| REBOOT["Reboot"]
  S -->|Automatic reboot eligible| TIMER["5-second countdown"]
  TIMER --> REBOOT
  classDef startPoint fill:#07545b,stroke:#77e6db,color:#f0fffc,stroke-width:3px
  classDef progress fill:#153858,stroke:#58a6ff,color:#f0f6fc,stroke-width:2px
  classDef success fill:#123c2a,stroke:#3fb950,color:#e7ffed,stroke-width:2px
  classDef error fill:#52212a,stroke:#f85149,color:#ffe8e5,stroke-width:2px
  classDef recovery fill:#342450,stroke:#b392f0,color:#f4ebff,stroke-width:2px
  classDef urlEntry fill:#123c42,stroke:#56d4ca,color:#e5fffc,stroke-width:2px
  class START startPoint
  class R recovery
  class U1,U2 urlEntry
  class P progress
  class E error
  class S success
```

| Action | Behavior |
| --- | --- |
| Arrows, Tab, number keys, Enter | Select and activate an action. `V` changes only the display log filter; Page Up/Down scrolls logs or error details. |
| Ctrl-C / Esc | On PROGRESS, open MENU without stopping work; MENU's Esc or Return returns to PROGRESS. On ERROR, Esc or Continue opens RECOVERY; Esc first closes full details if open. On a URL-entry screen, Esc returns to RECOVERY. On SUCCESS, Ctrl-C opens RECOVERY. |
| `D` on ERROR | Toggle between the log panel and full error details. |
| Shell or `S` | Opens a root recovery shell from PROGRESS, MENU, ERROR, RECOVERY, or SUCCESS. Servicing continues and automatic reboot waits while the shell owns the terminal; `exit` returns to the latest screen. A nonzero shell exit is only a warning. |
| Stream COSI URL / Install HC URL | Recovery accepts HTTP(S) URLs without embedded username or password. A new attempt reloads installer TOML. Explicit StreamDisk recovery skips local media; recovery HC checks it once without waiting. |
| Reboot / Shutdown | A running worker or uncertain outcome blocks power actions. Shutdown is not offered while the outcome is uncertain; a later attempt first checks Trident's servicing state before another write. |
| Automatic reboot | Starts five seconds after success **only** when enabled, work was performed, and Trident says a reboot is required. |

With `--system-console`, the installer uses tty1 for the UI and mirrors logs
to active serial consoles; serial consoles do not receive a TUI. `--plain`
uses line-oriented output instead of the full-screen display. Without an
interactive terminal, autorun still runs, but a failure exits with an error
instead of opening recovery. All received events are retained in the private
`/var/log/trident-installer.log`, regardless of display or serial filtering.
