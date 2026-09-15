# Installation

tailor is a single static binary. Install a prebuilt release, or build it from source, then point it at a container engine.

## Prebuilt release binary (recommended)

Releases publish static Linux musl binaries for `x86_64` and `aarch64`, each with a `.sha256` checksum. This snippet always fetches the **latest** release:

```bash
set -euo pipefail
target="x86_64-unknown-linux-musl" # or aarch64-unknown-linux-musl
base="https://github.com/frhuelsz/tailor/releases/latest/download"

curl -L -O "${base}/tailor-${target}"
curl -L -O "${base}/tailor-${target}.sha256"
sha256sum -c "tailor-${target}.sha256"
chmod +x "tailor-${target}"
sudo install -m 0755 "tailor-${target}" /usr/local/bin/tailor
```

The binary is fully static: it does not require glibc or OpenSSL on the target machine.

Each release also publishes a cosign signature bundle, an SBOM, and build provenance. To verify the
binary's signature and provenance (not just its checksum), follow the verification steps in the
[project README](https://github.com/frhuelsz/tailor#verifying-releases).

## From source

The crate is not published to crates.io yet, so install from git (or a local checkout):

```bash
cargo install --git https://github.com/frhuelsz/tailor tailor
# From a local checkout:
cargo install --path crates/tailor
```

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
