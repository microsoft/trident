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
use its Reboot button to boot the installed OS.

The generated ISO is `test/artifacts/installer_amd64_iso.iso`. This local build
uses a pre-existing regular Azure Linux 3.0.20260909 COSI (175 MB compressed,
4 GiB original disk). The launcher creates an 8 GiB disposable disk.

The launcher follows the ACL test runner's UEFI setup, but is Python. It creates
a new sparse qcow2 disk and private firmware variables for every run. No host
disk or shared host directory is exposed to the guest. Per-device `bootindex`
makes the hard disk first: a blank disk falls through to the ISO, and the
installed OS takes over after reboot.

```console
python3 test/run.py --display gtk --keep
python3 test/run.py --dry-run
```

`--keep` retains the disk, firmware variables and serial log for inspection.
Otherwise that run's private `.vm/` directory is removed after QEMU exits.
GTK uses the graphical installer console; serial status is captured separately.

Build prerequisites: Python 3.11+, Docker, Tailor and the cached
`azl3/trident-builder:latest` image. Trident binaries are built inside Azure
Linux rather than using workstation binaries with potentially incompatible
glibc. The normal repository RPM spec packages them. Tailor runs Image
Customizer 1.6 on an Azure Linux 3 minimal base.

Runtime prerequisites: QEMU, `qemu-img`, and matching OVMF code/variables.
KVM is used when available; otherwise QEMU emulation is used.

```console
python3 test/build.py --skip-rpms
python3 test/build.py --cosi /path/to/regular.cosi --prepare-only
```

Payloads, RPMs, ISO artifacts and VM disks are ignored by Git. This tests QEMU's
IDE CD-ROM path, not USB/BMC virtual-media detection.
