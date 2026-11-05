# TUI installer ISO

Tailor builds an Azure Linux 3 ISO with the `trident-installer` RPM and Trident's
gRPC daemon. The installer boots in autorun mode, mounts the `CDROM`-labelled
ISO read-only, and streams the staged COSI. It does **not** enter demo mode:
boot only with a disposable target disk. The success screen remains visible
until an operator chooses Reboot.

Stage matching Trident RPMs (including `trident-installer`) in `rpms/` and an
Azure Linux COSI in `cosi/payload.cosi`. These inputs are local and ignored by
Git. Use matching RPMs from `bin/RPMS/` after building the Trident RPMs,
and a COSI such as `artifacts/trident-testimage.cosi` from the existing image
builder. From the repository root:

```console
tailor --manifest tests/images/tailor.yaml validate
tailor --manifest tests/images/tailor.yaml build tui-installer \
  --output-dir tests/images/artifacts
```

The generated ISO is `tests/images/artifacts/tui-installer_amd64_iso.iso`.
Build this image with Tailor, not the legacy `testimages.py` CLI.
Use the existing virt-deploy and netlaunch tools to boot it in a VM with a new
disposable disk (at least as large as the COSI's target disk):

```console
mkdir -p test/.work
bin/virtdeploy create-one --namespace tui-installer-demo \
  --network 192.168.244.0/24 --disks 8 --mem 6 \
  --no-secure-boot --no-tpm \
  --netlaunch test/.work/tui-installer-netlaunch.yaml
bin/netlaunch --port 46119 \
  --config test/.work/tui-installer-netlaunch.yaml \
  --iso tests/images/artifacts/tui-installer_amd64_iso.iso
```

The ISO-only `--port` mode needs netlaunch with the fix in #909; older versions
exit on the firmware's first HTTP probe before the ISO can boot. Leave
netlaunch serving until the installer has loaded. In another terminal, run
`virsh --connect qemu:///system domdisplay tui-installer-demo-vm-0` to find
the SPICE address. Connect with a SPICE viewer; for a remote VM host, tunnel
that address's localhost port over SSH. When finished, stop netlaunch and run
`bin/virtdeploy clean --namespace tui-installer-demo` to remove only this VM,
its disk, and its libvirt network.

The checked-in configuration uses `filesystemLabel = "CDROM"` to match Image
Customizer ISO output; change it if the image is copied to differently labelled
media. `trident-installer` is optional in the RPM build, but its package must
be present in `rpms/` for this image to build.
