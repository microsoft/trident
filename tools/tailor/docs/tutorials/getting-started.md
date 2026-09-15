# Getting started

In this tutorial you will create one standalone image definition and render the Image Customizer invocation without building an image.

## 1. Install tailor

Use a release binary or install from git:

```bash
cargo install --git https://github.com/frhuelsz/tailor tailor
```

Check it:

```bash
tailor --version
```

Expected shape:

```text
tailor <version>+<commit>.<date>
```

## 2. Scaffold a standalone image

Create a new empty directory, then run:

```bash
mkdir solo-demo
cd solo-demo
tailor init solo simple
```

Expected output includes:

```text
created .../solo-demo/image.yaml
Scaffolded standalone image `solo`. Try: tailor validate
```

The `simple` template creates only `./image.yaml`; there is no `tailor.yaml`. In standalone mode tailor uses its built-in default Image Customizer toolchain: `mcr.microsoft.com/azurelinux/imagecustomizer:latest`.

## 3. Read the image definition

Open `image.yaml`:

```yaml
name: solo
outputs:
  - format: cosi
base:
  azureLinux:
    version: "3.0"
    variant: minimal-os
config:
  os:
    hostname: solo
    packages:
      install:
        - openssh-server
    services:
      enable:
        - sshd
```

Top-level keys are tailor's. Everything under `config:` is Image Customizer configuration and is passed through opaquely.

## 4. Validate

```bash
tailor validate
```

Expected shape:

```text
✓ solo                         1 cell(s) valid
```

## 5. Dry-run the build

```bash
tailor build --dry-run
```

Expected shape:

```text
1 cell(s) (dry-run)
...
imagecustomizer ... --config-file ... --output-image-format cosi ...
```

`--dry-run` renders the build plan — the container/Image Customizer invocation — and prints it without starting a container. It contacts no container engine, so it works with no Docker daemon. Remove `--dry-run` when you have Docker daemon access and want to build the artifact.

## Next step

Learn matrix builds in [Your first matrix](your-first-matrix.md).
