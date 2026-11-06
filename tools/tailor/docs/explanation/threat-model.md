# Threat model

This document states what tailor defends against, and — just as importantly —
what it does not. It bounds the scope of the safety guards in the codebase so
contributors know which risks are in scope for a fix and which are explicitly
out of scope.

## Trust boundary

tailor is a developer/CI build tool. It runs on a **trusted host**, under a
**trusted operator**, against **trusted configuration** authored by that
operator or their team. It builds OS images by driving Image Customizer (IC)
inside a container runtime, and it performs a small number of privileged
filesystem operations (chowning and removing build artifacts that IC writes as
root inside the container).

The security posture follows from that boundary:

- **In scope:** preventing *accidents* — a misconfiguration, a bad path, or a
  bug in tailor turning a routine build into destructive action against the
  host. The motivating example is a cleanup step that, given a pathological
  build directory, could `rm -rf` an unintended location.
- **Out of scope:** defending against a **malicious operator** or **hostile
  configuration**. Someone who can edit `tailor.yaml`/`image.yaml`, pass
  arbitrary flags, or run arbitrary commands on the host can already do anything
  tailor could; tailor does not attempt to sandbox them from themselves. In
  particular, tailor does not defend against symlink/TOCTOU races engineered by
  an attacker who already controls the build directory, nor against a
  deliberately malicious base image or IC toolchain image.

If your deployment does not fit "trusted host, trusted operator, trusted
config" — for example, if you build attacker-controlled configuration on shared
infrastructure — you need an additional isolation layer (a disposable VM or
container per build) around tailor; tailor alone is not that boundary.

## What tailor guards against

These guards defend against the accident class above and are considered
load-bearing — treat a regression in them as a serious bug:

- **Never operate on the filesystem root or a system directory.** Before any
  build directory, writable tools directory, or writable RPM source is bound
  into the privileged container, tailor refuses paths that resolve to `/`, to a
  well-known system directory (`/usr`, `/etc`, `/home`, `/var`, …) or `$HOME`, or
  that contain the current working directory. The janitor likewise refuses to
  bind `/` to reclaim a child. This is what stops a stray build directory from
  exposing the whole host to a recursive delete. See
  `crates/tailor-exec/src/guard.rs`.
- **Single build per output directory.** A build takes an advisory lock on its
  output directory, so two concurrent builds cannot race on the shared stamp,
  hash-cache, and artifact writes. See `crates/tailor-exec/src/lock.rs`.
- **Atomic state writes.** Build stamps, the hash cache, and the published
  artifact are written to a temporary file and renamed into place, so an
  interrupted build cannot leave torn state a later run would trust. See
  `crates/tailor-core/src/atomic.rs`.
- **Scoped privileged cleanup.** The janitor only chowns/removes the specific
  paths a build produced (the build directory's children, the image cache, the
  per-cell log), never arbitrary locations.

## Supply-chain pinning

Because a moving container tag is a reproducibility hazard (today's `:latest` is
not tomorrow's), tailor resolves and pins the digests of the images it runs —
the IC toolchain, tools-directory sources, base images, and the janitor image —
into `tailor.lock`. Pinning by digest is a **consistency and reproducibility**
measure under the trusted-host model, not a defense against a compromised
registry account; run `tailor lock` to freeze digests and `tailor update` to
refresh them deliberately.

## Reporting

If you find a way for tailor to damage the host *from within its intended
trust boundary* — that is, an accident-class escape that a normal operator with
ordinary configuration could trigger — please report it. Findings that require
a malicious operator or hostile configuration are, by the boundary above,
outside tailor's model.
