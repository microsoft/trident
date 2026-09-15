# Changelog

All notable changes to this project are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Release tags are component-scoped as `tailor-v<version>`; tags `v1.0.0` and
earlier use the bare `v<version>` scheme.

## [Unreleased]

## [1.0.2] - 2026-09-11

### Added

- `tailor build <slug>` now accepts a cell slug as a positional argument: it
  builds exactly that cell of its owning image, as shorthand for
  `tailor build <image> --cell <slug>`. Positionals may still be image names, and
  an unrecognized positional gives a clear "no matching image or cell slug" error.

### Fixed

- `tailor validate` (and other non-build verbs) no longer reject a tools-dir
  image when `runtime.buildDirBase` is unset. The build path already defaults
  `buildDirBase` to `<output>/.tailor/build`; the validate-time check was a stale
  leftover of the old "buildDirBase is required" rule.

## [1.0.1] - 2026-09-11

### Changed

- `runtime.buildDirBase` is now optional: it defaults to `<output>/.tailor/build`,
  so tools-dir images (which need a writable scratch dir) build with no
  host-specific path. The build-directory guard no longer requires a separate
  filesystem — it rejects `/`, system directories, and `$HOME` instead — since
  Image Customizer keeps its overlays and mounts within `--build-dir`/`--tools-dir`.

## [1.0.0] - 2026-09-10

First stable release. The `tailor` CLI is now covered by the
[compatibility policy](COMPATIBILITY.md); the library crates remain internal.

### Added

- Compatibility policy (`COMPATIBILITY.md`) and threat model
  (`docs/explanation/threat-model.md`).
- Workspace-level `previewFeatures` opt-in for not-yet-stable features; signing
  is gated behind `previewFeatures: [signing]`.
- `tailor notice` prints tailor's MIT license and the embedded third-party
  license notices for every linked dependency.
- Documented exit-code taxonomy: `2` usage error, `1` build failure, `130`
  interrupted (SIGINT).
- `--clones` now produces distinct artifacts (`<slug>_clone<n>`), each with its
  own stamp, and always rebuilds.
- Crash-safe atomic writes for build stamps, the hash cache, and published
  artifacts.
- Single-build-per-output-directory advisory lock.
- Release provenance: version/tag gate, test gate, cosign keyless signing,
  CycloneDX SBOM, and build provenance attestation; pinned toolchain and action
  digests.

### Changed

- `tailor lock` freezes the current pins (idempotent); `tailor update`
  re-resolves every input to its latest digest. The configured janitor image is
  now pinned in `tailor.lock`.
- `schemaVersion` is enforced: a version newer than tailor supports is rejected.
- The build fingerprint folds in a build-schema version, so artifacts correctly
  rebuild after a tailor upgrade that changes build semantics.

### Fixed

- A `-s`/`--cell` selection meant for a consumer no longer excludes (or errors
  on) a transitively required producer cell.

### Removed

- The inert `injectFiles` image field (rejected if used).

## [0.8.0] - 2026-09-09

### Added

- Inter-image `inputs:` embedding and interpolation.

## [0.7.0] - 2026-09-09

### Added

- Inter-image dependencies with image-as-base references and build ordering.

## [0.6.0] - 2026-09-09

### Added

- zstd post-build output compression via `compression:`.

## [0.5.0] - 2026-09-09

### Added

- `tailor convert` for workspace-free Image Customizer conversion.
- Extra parameter passthrough for Image Customizer flags.

### Fixed

- RPM source fingerprinting and writable RPM farm handling.
- Same-device build directory defaults for conversion.

## [0.4.0] - 2026-07-20

### Added

- `tailor export` for configs-only exports, including `--check`.
- `--build-dir-base` support for builds.

## [0.3.0] - 2026-07-14

### Added

- Versioned documentation site and release download documentation.

### Fixed

- Cleanup container target binding and native-architecture execution.

## [0.2.0] - 2026-07-10

### Added

- Managed tools directory support.
- Local container image support through pull policy.
- End-to-end signing flow and signing executor support.

### Changed

- Hardened Image Customizer container mounts and host path handling.
- Improved base hashing performance and observability.

## [0.1.0] - 2026-06-23

### Added

- Initial tailor CLI, runtime, documentation, and tests.

[Unreleased]: https://github.com/frhuelsz/tailor/compare/tailor-v1.0.2...HEAD
[1.0.2]: https://github.com/frhuelsz/tailor/compare/tailor-v1.0.1...tailor-v1.0.2
[1.0.1]: https://github.com/frhuelsz/tailor/compare/v1.0.0...tailor-v1.0.1
[1.0.0]: https://github.com/frhuelsz/tailor/compare/v0.8.0...v1.0.0
[0.8.0]: https://github.com/frhuelsz/tailor/compare/v0.7.0...v0.8.0
[0.7.0]: https://github.com/frhuelsz/tailor/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/frhuelsz/tailor/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/frhuelsz/tailor/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/frhuelsz/tailor/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/frhuelsz/tailor/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/frhuelsz/tailor/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/frhuelsz/tailor/releases/tag/v0.1.0
