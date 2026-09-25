# Installation

tailor is a single static binary. Build it from source, then point it at a container engine.

## From source

The crate is not published to crates.io yet. Install it straight from the monorepo:

```bash
cargo install --git https://github.com/microsoft/trident tailor
```

Or from a local checkout of this directory:

```bash
cargo install --path crates/tailor
```

The binary is fully static: it does not require glibc or OpenSSL on the target machine.

## Verify

```bash
tailor --version
```

It prints the SemVer version plus build metadata, e.g. `tailor <version>+<commit>.<date>`.

## Runtime requirement: a container engine

tailor runs the Azure Linux Image Customizer inside a container, so a running **Docker** (or Podman) daemon is required at build time — see [Select a container engine](how-to/select-a-container-engine.md). The binary itself has no other runtime dependencies.

`tailor validate`, `matrix`, `slugs`, and `render` work without a daemon; only `build` (and the other execution verbs) need one.

## Next steps

- [Getting started](tutorials/getting-started.md) — scaffold and render your first image.
- [Your first matrix](tutorials/your-first-matrix.md) — expand one definition into many cells.
