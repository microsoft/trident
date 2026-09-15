# Compatibility policy

This document defines what tailor promises to keep stable across releases, and
what it explicitly does not. It takes effect at **1.0**. tailor follows
[Semantic Versioning](https://semver.org/): within a `1.x` line, the guarantees
below hold; a change that breaks one of them is reserved for a new major
version.

The **`tailor` command-line binary is the stable public interface.** The
library crates (`tailor-core`, `tailor-config`, `tailor-exec`,
`tailor-resolve`, `tailor-sign`) are internal implementation detail: they are
published only to build the binary and may change in any release. Do not depend
on them as a library API.

## What is stable (semver-governed)

### Command-line surface

- Existing subcommands and their flags keep their names and meaning. Flags are
  not removed or repurposed within a major version; new subcommands and new
  optional flags may be added (a minor, backward-compatible change).
- Hidden/experimental flags (not shown in `--help`) are **not** covered — they
  may change or disappear at any time.

### Configuration schema

- The `tailor.yaml` / `image.yaml` schema is versioned by `schemaVersion`.
  tailor accepts any `schemaVersion` from `1` up to the highest version it
  supports, and rejects anything newer with a clear error. Images and fragments
  inherit the workspace `schemaVersion`.
- Within a major version, existing fields keep their names, types, and meaning;
  new optional fields may be added. A field that is renamed or given
  incompatible semantics comes with a `schemaVersion` bump and a documented
  migration path.

### Artifact naming

- The cell **slug** and the published **artifact filename** derived from it are
  part of the contract: a given configuration produces the same slug and the
  same output filename across `1.x` releases, so downstream automation can rely
  on the paths. Clones extend this predictably as `<slug>_clone<n>`.

### Exit codes

tailor uses a documented exit-code taxonomy so scripts and CI can branch on the
class of failure:

| Code | Meaning |
| --- | --- |
| `0` | Success. |
| `1` | Build failure — an operational error (the engine failed, IO broke, a resolution or signing step errored). |
| `2` | Usage error — invalid configuration or an invalid request (a bad selector, an unknown image, a dependency cycle, bad arguments). |
| `130` | Interrupted — the run was cancelled by `Ctrl+C` / `SIGTERM` (`128 + SIGINT`). |

## What is **not** promised (may change in any release)

The following are intentionally left outside the 1.0 contract so tailor can keep
improving them:

- **Human-readable stdout/stderr.** Log lines, progress output, and diagnostic
  wording are for people, not parsing. A stable machine-readable output mode is
  future work; until it ships, do not screen-scrape tailor's output.
- **Lockfile format.** `tailor.lock` is generated and consumed by tailor; its
  on-disk layout may change. Regenerate it with `tailor lock` / `tailor update`
  rather than editing or parsing it.
- **Minimum Supported Rust Version (MSRV).** tailor is distributed as a
  prebuilt binary; the Rust toolchain used to build it may move at any time.
- **Deprecation policy.** There is not yet a formal warn-before-remove cadence.
  Breaking changes are gated on a major version bump, but the lead time and
  warning mechanics are not guaranteed.
- **Preview features.** Anything opted into via `previewFeatures:` in
  `tailor.yaml` (currently `signing`) is explicitly outside these guarantees:
  its schema, flags, and behavior may change or be removed in any release until
  the feature is promoted out of preview and documented as stable.

## Releases and verification

Release tags are component-scoped as `tailor-v<version>` (e.g. `tailor-v1.0.2`).
Each release publishes static binaries with cosign signatures, checksums, an
SBOM, and build provenance; see the project README and
[Installation](docs/installation.md) for how to verify them.

## Internal (never stable)

- Library crate APIs (see above).
- Build stamps, the hash cache, and other files under the output state
  directory.
- Exact error message text (the exit-code *class* is stable; the wording is
  not).
