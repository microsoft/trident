# Licensing and third-party notices

tailor is licensed under the [MIT License](../../LICENSE). Because tailor is distributed as a
statically linked binary, every dependency it links is compiled into that binary, so their licenses
travel with it. This page describes how tailor stays compliant.

## Policy: what dependencies are allowed

tailor restricts its dependency tree to **permissive, attribution-style licenses** — no copyleft.
The policy lives in [`deny.toml`](../../deny.toml) and is enforced in CI by
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny): a pull request that introduces a
dependency under a license outside the allow-list fails the `cargo-deny` check until reviewed.

The allow-list is:

`MIT`, `Apache-2.0`, `Apache-2.0 WITH LLVM-exception`, `BSD-2-Clause`, `BSD-3-Clause`, `ISC`, `Zlib`,
`Unicode-3.0`, `BSL-1.0`, `Unlicense`, `CDLA-Permissive-2.0`.

`cargo-deny` also checks for known security advisories, yanked releases, and off-registry sources.
Run it locally with:

```bash
cargo install cargo-deny --locked
cargo deny check
```

## Attribution: the `tailor notice` command

Permissive licenses require reproducing each dependency's copyright and license text when you
distribute the software. tailor satisfies this with an embedded notice:

- At build time, `crates/tailor/build.rs` walks the **normal-dependency closure** of the binary
  (skipping dev- and build-dependencies, which do not ship), reads each crate's license and notice
  files from the local cargo source cache, and writes an aggregated notice into `OUT_DIR`.
- The binary embeds that file with `include_str!`, and `tailor notice` prints tailor's own MIT
  license followed by every dependency's name, version, SPDX identifier, and full license text.

Because the notice is generated from the resolved `Cargo.lock` on every build, it can never drift
from the dependency set — there is no checked-in copy to keep in sync. The license-collection tool
(`cargo_metadata`) is a **build-only** dependency, so it is not linked into the shipped binary.

To archive the third-party attributions alongside a release:

```bash
tailor notice > THIRD-PARTY-NOTICES.txt
```

## Changing the copyright holder

`LICENSE` and the `authors`/`repository` fields in `Cargo.toml` name the copyright holder. Update
them if ownership changes (for example, when moving the project into another organization).
