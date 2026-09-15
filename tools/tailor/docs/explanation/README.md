# Explanation

Explanation pages help you understand tailor's model and design choices. Unlike
the how-to guides, they are meant to be read rather than followed.

## Start here (the model)

- [Concepts](concepts.md) — the core vocabulary: images, matrices, axes, cells,
  slugs, fragments, and toolchains. Read this first.
- [Merge model](merge-model.md) — how base config and per-axis fragments combine,
  and the precedence rules that decide the final cell config.
- [Target architectures](target-architectures.md) — how the reserved `arch` axis
  selects the target platform and base image, and how the effective arch is resolved.
- [Base images](base-images.md) — where a cell's starting image comes from and the
  ways to declare one.

## Design and internals (contributors)

- [Architecture](crate-architecture.md) — the crate layout and dependency
  direction of the tailor workspace.
- [Design rationale](design-rationale.md) — why tailor is shaped the way it is,
  and the trade-offs behind the key decisions.

## Security and compliance

- [Threat model](threat-model.md) — the trust boundaries and what tailor does to
  keep a build from reaching the host.
- [Licensing and third-party notices](licensing.md) — tailor's license and how it
  reports the licenses of its dependencies.
