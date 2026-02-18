# Trident Container Test Image

This image an extension of the base bare metal images and includes various helper utilities along with Docker runtime. It also adds openssh-server to allow for remote access.

## Building

To build the base image and per-partition compressed images, run:

```bash
make trident-container-testimage
```

It will populate the partition files as follows:

```text
build/trident-container-testimage
├── esp.raw.zst
└── root.raw.zst
```
