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
The checked-in configuration uses `filesystemLabel = "CDROM"` to match Image
Customizer ISO output; change it if the image is copied to differently labelled
media. `trident-installer` is optional in the RPM build, but its package must
be present in `rpms/` for this image to build.
