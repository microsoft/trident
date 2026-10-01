# Convert an image to another format

`tailor convert` is a one-shot wrapper over `imagecustomizer convert`: give it an image file and a
target format and it produces the converted image — **no `tailor.yaml`, no `image.yaml`, no config**.
It runs Image Customizer in a container for you and leaves the output owned by your user (sudo-free,
via the same janitor as `tailor build`).

```bash
tailor convert ./disk.qcow2 --to raw
# → ./disk.raw
```

By default the output is written **beside the input**, with the target format's extension. Use `-o`
to choose a path (its parent directories are created as needed):

```bash
tailor convert ./disk.vhdx --to vhd-fixed -o ./out/gallery.vhd
```

## Supported formats

`--to` accepts the formats IC `convert` supports (it reformats an already-built image, so the
OS-build-only formats `iso`/`pxe-*` are not available):

`vhd`, `vhd-fixed`, `vhdx`, `qcow2`, `raw`, `cosi`, `baremetal-image`.

## Options

| Flag | Meaning |
| --- | --- |
| `--to <FORMAT>` | The target format (required). |
| `-o, --output <PATH>` | Output path (default: the input's name with the target extension, beside the input). |
| `--container <REF>` | The Image Customizer image to run (default: `mcr.microsoft.com/azurelinux/imagecustomizer:latest`). |
| `--arch <ARCH>` | `amd64` (default) or `arm64` — drives `--platform linux/<arch>`. |
| `--build-dir-base <PATH>` | Host base for IC's scratch (default: a unique dir under the system temp dir). Must not be `/`. |
| `--dry-run` | Print the container invocation without running it (no engine needed). |

The global engine flags apply too: `--engine docker|podman|auto` and `--host <endpoint>`.

## Preview the command

`--dry-run` renders the exact `docker`/`podman` invocation without running anything:

```bash
tailor convert ./disk.qcow2 --to raw --dry-run
```

## Notes

- The input must be a **local file** — IC `convert` takes an `--image-file`, not a registry
  download.
- `convert` only reformats the disk container; it does not customize the OS. To change the image's
  contents, use `tailor build` (or `imagecustomizer customize`).
- The output directory cannot be the exact directory you're running from *when you point `-o` at it*
  — a subdirectory (the default "beside the input" when the input is in a subdirectory, or any
  `-o subdir/…`) is always fine.
