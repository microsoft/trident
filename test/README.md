# Real installer ISO test

Build an Azure Linux installer ISO containing the optional `trident-installer`
RPM from this branch and an existing **non-ACL** COSI. The COSI is reused, not
rebuilt.

```console
python3 test/build.py --cosi /path/to/regular.cosi
python3 test/run.py --display gtk
```

The payload is copied into the ISO's `cosi/` folder. No HC is supplied, so
autorun chooses StreamDisk. `reboot = false` leaves the success screen visible;
use its Reboot button to check what the installed disk boots.
`serialMode = "logs"` routes INST and TRIDENT log records to the serial
console as `MM:SS [SOURCE:LEVEL] message`, up to `serialVerbosity` (default
`debug`), independently of TUI verbosity. Multiline messages repeat the prefix
on each line instead of printing a literal `\n`.
This ISO suppresses the serial getty prompt and normal kernel/systemd status
output; UEFI firmware output may still appear before the installer starts.
With only a serial console active, autorun runs without a TUI and honors
`autorun.reboot`; missing inputs or an already-present image produce tagged
errors instead of waiting for a keyboard response.
The ISO disables `getty@tty1.service` and logind's automatic virtual gettys so
they cannot reclaim the installer's graphical keyboard after startup. The
installer mirrors source-discovery stages to the serial log; an idle-looking
progress screen can be diagnosed there without sending keys to the guest.

The generated ISO is `test/artifacts/installer_amd64_iso.iso`. This local build
uses a pre-existing regular Azure Linux 3.0.20260909 COSI (175 MB compressed,
4 GiB original disk). The launcher creates an 8 GiB disposable disk.

The launcher follows the ACL test runner's UEFI setup, but is Python. It creates
a new sparse qcow2 disk and private firmware variables for every run. No host
disk or shared host directory is exposed to the guest. Per-device `bootindex`
makes the hard disk first: a blank or non-bootable disk falls through to the ISO.
For this cached `regular.cosi`, the installer successfully streams the image,
but a subsequent UEFI boot reports a missing `\\EFI\\BOOT\\grubx64.efi` even
after ejecting the ISO. This fixture verifies installation, **not** a bootable
target OS; choose a known-bootable COSI to test both. If the ISO remains attached,
the repeat-install guard prevents a second stream.

```console
python3 test/run.py --display gtk --keep
python3 test/run.py --dry-run
```

`--keep` retains the disk, firmware variables and serial log for inspection.
Otherwise that run's private `.vm/` directory is removed after QEMU exits.
The launcher follows tagged INST and TRIDENT serial records in the same
terminal, including TRACE when `serialVerbosity = "trace"`, without forwarding
firmware/GRUB escape sequences that could redraw your terminal. The complete
raw serial capture stays
in the indicated `serial.log` file while the VM runs; use `--keep` to retain it
after exit. GTK uses a separate graphical window for the interactive console.
Do not rebuild over an ISO still attached to a running VM. Use
`python3 test/build.py --output-dir test/.work/next-iso` and then pass
`--iso test/.work/next-iso/installer_amd64_iso.iso` to `run.py` to test a
replacement without changing the running guest's CD-ROM.

## GPT USB installation media

The installer finds a filesystem by label, so it can read COSI files from an
optical ISO9660 filesystem or a GPT USB partition (FAT32 in this fixture).
`filesystemLabel = "TRD_INSTALL"` fits FAT32's 11-character label limit.
Image Customizer ISO output is labelled `CDROM`. To build the USB test ISO,
temporarily set `filesystemLabel = "TRD_INSTALL"` in
`test/installer/files/installer.toml`, run
`python3 test/build.py --skip-rpms --output-dir test/.work/gpt-usb-ready`,
and restore `filesystemLabel = "CDROM"` afterward. Keep the two media labels
distinct to avoid ambiguous discovery.

```console
python3 test/make_usb.py --cosi test/installer/cosi/payload.cosi
python3 test/run.py --iso test/.work/gpt-usb-ready/installer_amd64_iso.iso \
  --usb-media test/.work/installer-usb.raw --vnc --keep
```

`make_usb.py` creates only a local, sparse 768 MiB GPT/FAT32 image under
`test/.work`. It uses Docker's cached builder for `sfdisk` and mtools to
populate `cosi/payload.cosi`; it does not repartition a host device.
`run.py` attaches it as a read-only USB mass-storage device. The USB is smaller
than the COSI's 4 GiB original disk, so Trident cannot select it as an install
target. A larger USB that could be the smallest eligible disk is rejected by
the installer preflight rather than being overwritten; the StreamDisk RPC
does not yet accept a target disk override.
This fixture exercises a Rufus-style GPT/FAT32 layout, not Rufus itself or
NTFS; non-FAT media require filesystem support in the live OS.

## Over SSH, including from a Wayland desktop

On the VM host:

```console
./test/run.py --vnc --keep
```

On your local desktop, open a second terminal:

```console
ssh -N -L 5901:127.0.0.1:5901 USER@HOST
```

Connect a VNC client such as GNOME Connections or Remmina to
`vnc://127.0.0.1:5901`. QEMU binds VNC to loopback only; the SSH tunnel carries
the connection. The guest stays paused until the viewer connects, so there is
no setup race. The screen is initially black while the guest is paused; boot
begins automatically when the viewer connects. No X11 forwarding or desktop
environment changes are needed.
Use `--vnc 2` and tunnel TCP 5902 if display 1 is already occupied.

## Build prerequisites

Build prerequisites: Python 3.11+, Docker, Tailor and the cached
`azl3/trident-builder:latest` image. Trident binaries are built inside Azure
Linux rather than using workstation binaries with potentially incompatible
glibc. The USB image fixture also needs mtools (`mformat`, `mmd`, `mcopy`).
The normal repository RPM spec packages them. Tailor runs Image
Customizer 1.6 on an Azure Linux 3 minimal base.

Runtime prerequisites: QEMU, `qemu-img`, and matching OVMF code/variables.
KVM is used when available; otherwise QEMU emulation is used.

```console
python3 test/build.py --skip-rpms
python3 test/build.py --cosi /path/to/regular.cosi --prepare-only
```

Payloads, RPMs, ISO artifacts and VM disks are ignored by Git. This tests QEMU's
IDE CD-ROM path, not USB/BMC virtual-media detection.
