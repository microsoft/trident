# Copilot Agent Instructions

These instructions apply to Copilot agents working in this repo — both agents
writing code and agents reviewing pull requests. The PR-review rules below are
review-only. The "Nits" and "Architecture & structural soundness" sections
apply to **both** writers (follow them when generating new code) and
reviewers (see [Reviewer etiquette](#reviewer-etiquette-nits--architecture)
for when to surface them).

## PR Review Scope

- Only comment on issues that are **specific to the diff** (avoid generic best-practice reminders).
- Avoid repeating the same point across multiple files. If one example demonstrates a pattern, mention it once and reference the pattern.
- **ALWAYS check previous reviews** before commenting. Do NOT repeat points that have already been made in previous reviews if they have been acknowledged, dismissed, or closed.

### What to focus on (in priority order)

1. Correctness and logic bugs
2. Security issues (input validation, authz/authn, secrets, injection)
3. Performance regressions (hot paths only)
4. API/contract changes and backward compatibility
5. Test gaps only when risk is high or behavior changed

## What to avoid

- Do NOT suggest stylistic refactors unless they fix a bug or prevent a clear maintenance issue.
- Do NOT request documentation unless public APIs changed.
- Do NOT comment on naming unless it causes real ambiguity.
- Do NOT suggest "add null checks" if the code is already guarded or types guarantee non-null.

### Output style

- Prefer fewer, higher-signal comments.
- Use this structure when leaving feedback:
    - **Issue** (why it matters)
    - **Evidence** (where in diff / what behavior)
    - **Suggestion** (concrete fix)

## Nits (Rust, applies to writers and reviewers)

Trident formats with `rustfmt` (workspace-wide — see `rustfmt.toml`).
Everything below is in addition to `cargo fmt` and is **aspirational**: real
files don't follow every rule 100% of the time. A counter-example in the tree
is not a license to ignore the rule in new code.

### Imports

1. **One `use` statement per top-level path, with a brace-tree for everything
   under it.** Never write two `use` lines that share a root:

    ```rust
    // BAD
    use std::fs;
    use std::path::PathBuf;

    // GOOD
    use std::{fs, path::PathBuf};
    ```

    So the `std` block is a single `use std::{…};`, each external crate is a
    single `use somecrate::{…};`, each workspace-local crate is a single
    `use workspacecrate::{…};`, and `crate::` and `super::` each get exactly
    one `use` per group.

2. **File ordering of imports.** Order the top-of-file import region in groups
   separated by a single blank line, alphabetical within each group:
    1. `use std::{…};`
    2. external crates (`anyhow`, `serde`, `tokio`, …)
    3. workspace-local crates (`osutils`, `sysdefs`, `trident_api`,
       `trident-proto`, …)
    4. `use crate::{…};`
    5. `use super::{…};`

    Then a blank line, then **submodule declarations**:

    ```rust
    mod mysubmodule;
    mod othermodule;
    ```

    Then, **only when necessary**, submodule imports:

    ```rust
    use mysubmodule::Foo;
    use othermodule::bar;
    ```

    A full example:

    ```rust
    use std::{collections::HashMap, path::PathBuf};

    use anyhow::{Context, Error};
    use log::{debug, info};

    use osutils::{files, mountpoint};
    use trident_api::error::{TridentError, TridentResultExt};

    use crate::engine::EngineContext;

    use super::Subsystem;

    mod inner;
    mod helpers;

    use inner::InnerThing;
    ```

3. **Test modules:** the very first line inside `mod tests { … }` is
   `use super::*;` on its own, followed by a blank line and then the standard
   import groups above (rules 1 and 2).
4. **Uppercase identifiers — import directly.** Types, enums, traits,
   structs: `use foo::Bar;` → `Bar::new(…)`.
5. **Lowercase identifiers (free functions) — import the parent module, not the
   function.** `use osutils::files;` → `files::write(…)`, **not**
   `use osutils::files::write;` → `write(…)`. This keeps call sites
   self-documenting and avoids name collisions.
6. **Macros — import directly**, even though their names are lowercase:
   `use anyhow::{bail, ensure};` → `bail!(…)`, never `anyhow::bail!(…)` at the
   call site. Same for `log::{debug, info, warn, error, trace}`.
7. **Strictly prefer imports over full paths at the call site.** Never:

    ```rust
    // BAD
    let a = std::submodule::Type::new(...);
    ```

    Always:

    ```rust
    use std::submodule::Type;
    // …
    let a = Type::new(...);
    ```

    When two imports would collide on the same name, prefer a sensible
    `use … as …` alias over reaching for a full path — e.g. when multiple
    `Context` types are in scope, `use tera::Context as TeraCtx;` and then
    `TeraCtx::new(…)` at the call site. There is no fixed naming scheme for
    aliases; pick a short, locally-readable name case by case. Reach for a
    fully-qualified path only when even an alias would be misleading
    (e.g. a one-off use where the full path is the clearest documentation).

8. **When a crate provides its own `Result` alias and a file uses it more
   than ~5 times, prefer importing the alias.** Example: in a file doing a
   lot of IO,

    ```rust
    use std::io::Result as IoResult;

    fn my_func() -> IoResult<Type> { … }
    ```

    over

    ```rust
    fn my_func() -> Result<Type, IoError> { … }
    ```

    For imports that overload language primitives (`Result`, `Error`,
    `Option`), always alias — never shadow the prelude name in a `use`
    without an `as`. Alias naming is case-by-case (`IoResult`, `TeraCtx`, …).

    **Exception: `anyhow`.** Trident handles `anyhow` errors explicitly —
    import `anyhow::Error` and spell `Result<T, Error>` out:

    ```rust
    use anyhow::Error;

    fn my_fn() -> Result<T, Error> { … }
    ```

    Don't alias `anyhow::Result`.

### Visibility & module layout

9. **Default to the strictest visibility that compiles.** New items start
   private (`fn`, `struct`), graduate to `pub(super)`, then `pub(crate)`, and
   only become `pub` when they intentionally cross the crate boundary. Be
   especially skeptical of `pub` that creates a dependency between distant
   modules — a `pub(crate)` re-export at `lib.rs` is usually a better answer
   than reaching deep into a submodule from elsewhere.

### Error handling

10. **Domain errors are `thiserror` enums in `trident_api::error`** (e.g.
    `InitializationError`, `InvalidInputError`, `ServicingError`,
    `InternalError`). Prefer adding a variant to an existing enum over
    introducing a new one. Variants use `#[serde(rename_all = "kebab-case")]` on
    the enum and a clear `#[error("…")]` message.
11. **Lift `anyhow`/`Result<_, E>` into `TridentError` with
    `.structured(<ErrorKind>)`** and attach human context with `.message("…")`
    (both from `TridentResultExt`/`ReportError`). Once a result is a
    `TridentError`, prefer `.message(…)` over `.context(…)`.
12. **`anyhow::Result` is fine in helper modules** (`osutils`, subsystem
    internals) whose callers handle errors with `anyhow` already. Don't return
    `anyhow::Error` from APIs whose callers need to discriminate variants —
    return a structured `TridentError` so the variant is preserved end-to-end.
13. **Avoid `unwrap()`/`expect()`/`panic!` in non-test code.** Accepted
    patterns: (a) lift to `TridentError` via `.structured(…).message(…)`;
    (b) `.expect("invariant: …")` documenting a static invariant.
14. **Use `anyhow::Context` to build informative error chains** when each layer
    adds genuinely new information (which subject failed, which path, which
    iteration). It is **not** required at every `?` — redundant context like
    `.context("failed to do thing")` on a function literally named `do_thing`
    is noise. The point is to make authors think about whether the next reader
    of the error can reconstruct what went wrong.
15. **When context is a `format!(...)`, use `.with_context(|| format!(…))`
    instead of `.context(format!(…))`** so the string is only built on the
    error path. Plain string literals stay on `.context("…")`.

### Logging

16. **Use the `log` crate** (`use log::{debug, info, warn, error, trace};`) for
    application logging. `tracing` is reserved for the existing
    `tracestream`/journald wiring — don't introduce new `tracing::info!`
    callsites in code that's already using `log`.

### Tests

17. **Inline `#[cfg(test)] mod tests { … }`** at the bottom of the file under
    test (vs. separate `tests/` files), unless the test crosses crate
    boundaries.
18. **Prefer `.unwrap()`/`.unwrap_err()` over `assert!(x.is_ok())` /
    `assert!(x.is_err())`** — the panic surfaces the underlying error.
    For variant assertions: `assert!(matches!(err, ErrorKind::Foo { .. }), "got {err:?}")`.
19. **Use `indoc!`/`formatdoc!` for multi-line literals** in tests; both are
    already on the workspace dep list.

### Serde / config types (`trident_api::config`)

20. **Public config types derive `Serialize, Deserialize, Debug, Default,
Clone, PartialEq, Eq`** (in that ordering when adding new ones) and use
    `deny_unknown_fields`. **Casing convention:** structs use
    `#[serde(rename_all = "camelCase", deny_unknown_fields)]` for their fields;
    enums use `#[serde(rename_all = "kebab-case", deny_unknown_fields)]` for
    their variants. (Same rule applies to any serialized type outside `config`,
    e.g. status / RPC payloads.) Optional fields:
    `#[serde(default, skip_serializing_if = "Option::is_none")]`. Non-Option
    defaults: `#[serde(default, skip_serializing_if = "is_default")]`.
21. **Gate `JsonSchema` behind the `schemars` feature:**
    `#[cfg_attr(feature = "schemars", derive(JsonSchema))]`, with the import
    `#[cfg(feature = "schemars")] use schemars::JsonSchema;`.

### Cargo & workspace hygiene

22. **All third-party deps come from `[workspace.dependencies]`** — every
    crate's `Cargo.toml` says `foo = { workspace = true }`, never an inline
    version. New deps are added to the root `Cargo.toml` first.
23. **Workspace-local deps use a `path = "..."` reference** (see existing
    `# Local Crate Dependencies` blocks).

### Misc Rust idioms

24. **Inline format args** (`format!("{name}")`, `info!("done: {count}")`) over
    positional (`format!("{}", name)`). Fall back to positional only when the
    expression isn't a bare identifier or a simple `expr.field` /
    `expr.method()`.
25. **Prefer `impl AsRef<Path>` over `&Path` / `&PathBuf`** for function
    arguments that just need to read a path, unless there is a concrete reason
    not to (e.g. you actually need a `&Path` to feed a sibling API in a hot
    loop, or you want to deliberately constrain callers). Same principle for
    `impl AsRef<str>` / `impl AsRef<[u8]>` where appropriate. Inside the
    function body, immediately bind once: `let path = path.as_ref();`.
26. **No magic numbers or magic strings.** Names like `0o755`, `300`,
    `"/etc/trident/datastore"`, `"trident-overlay"` should be `const`s with
    explanatory names, scoped as tightly as the use justifies. Module-local
    constants live at the top of the file; cross-module constants belong in
    `trident_api::constants` (or `osutils::*`'s relevant module). Reach for
    `trident_api::constants::internal_params::*` for tunables that are also
    surfaced as host-config knobs.
27. **Comments explain _why_, not _what_.** A doc that restates the function
    name is noise; a doc that names the invariant or links to the relevant
    design section is signal.
28. **Aim for shorter expressions that remain readable.** Prefer a sensible
    `match` over nested `if`/`else if` chains, especially when branching on
    multiple values at once (`match (a, b) { … }`). Lean on iterators
    (`.iter().filter().map().collect()`) when they're clearer than a manual
    loop, but prefer loops when an iterator chain would become too cumbersome
    or cryptic and a loop provides a more self-documenting solution. Reduce
    duplication with a local closure (`let normalize = |s: &str| …;`) instead
    of repeating a 3-line block four times. Avoid verbose blocks that can be
    expressed more succinctly — but stop short of cleverness that hurts the
    next reader (one-line iterator chains with side effects, deeply nested
    closures). The bar is: a reasonable reviewer should read the code at
    roughly the same speed as a more verbose version, with fewer tokens to skim.

## Architecture & structural soundness

The trident workspace is layered. New code belongs in the **lowest** layer
where it logically fits. Adding code at the wrong layer is the most common
way to accumulate cross-cutting tangles, and it is one of the few "stylistic"
issues that **is** worth flagging in review.

### Crate map (lowest to highest)

| Crate                   | Layer                    | Owns                                                                                                                                                                                                                                                                      | Depends on                         |
| ----------------------- | ------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------- |
| `sysdefs`               | System definitions       | High-level definitions of generic computing concepts — machines, operating systems, sometimes specific OSes. Holds the axiomatic types/enums/constants (architectures, filesystems, partition-type GUIDs, TPM2, OS UUIDs). **No I/O, no behavior, no dependencies.**      | nothing                            |
| `trident-proto`         | Wire types               | gRPC/protobuf-generated types for Trident's control surface.                                                                                                                                                                                                              | tonic/prost                        |
| `trident_api`           | Public API / contract    | **How Trident talks to the world.** The wire/file contract callers use to _tell Trident what to do_ (`HostConfiguration` + validation) and what Trident _reports back_ (`HostStatus`, `TridentError`/`ErrorKind`). Also cross-crate constants (`trident_api::constants`). | `sysdefs`, `trident-proto`         |
| `osutils`               | OS-interaction wrappers  | Thin, single-purpose wrappers around system tools/syscalls (`lsblk`, `mkfs`, `mount`, `systemd`, `grub`, `repart`, `chroot`, `efivar`, …). **No business logic, no policy decisions.**                                                                                    | `sysdefs`, `trident_api`           |
| `osmodifier`            | OS configuration applier | Native-Rust replacement for the Go `osmodifier`; applies OS-config changes (hostname, modules, services, users, selinux, grub) under a chrooted root.                                                                                                                     | (largely standalone)               |
| `trident`               | Business logic / binary  | The actual servicing engine — orchestrator, subsystems (`storage`, `osconfig`, `network`, `selinux`, `extensions`, `initrd`, `esp`, `management`, `hooks`), `engine` (clean install / A-B / runtime update / rollback), CLI, server, datastore, logging, gRPC client.     | everything below                   |
| `trident-acl-agent`     | Update client (Harpoon)  | Omaha-protocol client that fetches updated `HostConfiguration` documents for Trident.                                                                                                                                                                                     | `trident-proto`                    |
| `docbuilder`            | Tooling                  | Builds markdown docs from the `HostConfiguration` schema, the CLI definitions, and architecture pages. **Not on the runtime path.**                                                                                                                                       | `trident_api` (with feature flags) |
| `pytest` / `pytest_gen` | Test tooling             | Proc-macro + runtime that lets Rust functions register themselves as Python `pytest` cases for functional/E2E test discovery.                                                                                                                                             | —                                  |

### Where does this code go?

Apply these questions in order; stop at the first "yes":

1. **Is it a generic, axiomatic definition of a computing concept** — a
   machine/architecture/OS/filesystem/partition/TPM concept — with no
   behavior and no dependencies? → `sysdefs`.
2. **Is it part of the on-the-wire Trident control protocol?** →
   `trident-proto` (regenerated from `proto/`).
3. **Is it part of how Trident talks to the world?** — the schema callers
   use to _tell Trident what to do_ (Host Configuration / validation) or
   what Trident _replies_ (Host Status, error contract), or a constant
   shared across multiple crates → `trident_api`.
4. **Is it a thin wrapper around a system tool, syscall, or `/proc`/`/sys`
   read?** (e.g. "run `mkfs.ext4`", "parse `lsblk -J`", "read efivars") →
   `osutils`. The litmus test: a non-trident project should plausibly be able
   to use this function. **No subsystem-level decisions live in `osutils`.**
5. **Does it apply OS configuration under a chrooted root** (hostname,
   modules, services, users, selinux, grub)? → `osmodifier`.
6. **Does it make a Trident-specific decision** ("should we A/B?", "what
   order do subsystems run in?", "is this rollback safe?") or sequence
   multiple `osutils` calls into a workflow? → `trident` (under
   `engine/`, `subsystems/`, `orchestrate.rs`, …).

### Layering rules (enforced socially, not by the compiler today)

- **Lower layers do not import higher layers.** `osutils` does not depend on
  `trident`. `trident_api` does not depend on `osutils`. `sysdefs` depends on
  nothing.
- **`osutils` is policy-free.** A function in `osutils` answers
  "how do I invoke `mkfs.ext4` with these options?", not "should we run
  `mkfs.ext4` here?". Decision-making belongs in `trident`'s subsystem /
  engine code.
- **`trident_api` is Trident's public surface — keep it behavior-free.** It
  defines the types callers send in (Host Configuration), the types Trident
  sends back (Host Status, structured errors), and their validation. It does
  not execute servicing, perform I/O, or know about internal subsystems. If
  you're adding I/O or a workflow step to `trident_api`, it belongs in
  `trident` instead.
- **`trident-acl-agent` (`harpoon`) is standalone.** It is a separate binary
  that talks to Trident over the gRPC contract; do not couple it to
  `trident`'s internals.
- **`docbuilder` and `pytest`/`pytest_gen` are dev-only.** They must not
  appear in the runtime dependency graph of `trident` or `trident-acl-agent`.

### Code reuse (do this before writing anything)

Before introducing a new utility, check — in this order:

1. **`osutils` first.** It already covers `blkid`, `block_devices`,
   `bootloaders`, `chroot`, `container`, `dependencies`, `df`, `e2fsck`,
   `efibootmgr`, `efivar`, `encryption`, `exe`, `files`, `filesystems`,
   `findmnt`, `grub`, `hostname`, `installation_media`, `lsblk`, `lsof`,
   `machine_id`, `mdadm`, `mkfs`, `mkinitrd`, `mount`, `mountpoint`,
   `netplan`, `osrelease`, `overlay`, `path`, `pcrlock`, `repart`,
   `resize2fs`, `scripts`, `sfdisk`, `swap`, `systemd`, `tabfile`, `tune2fs`,
   `udevadm`, `uki`, `uname`, `veritysetup`, `virt`, `wipefs`. If your task
   smells like one of these, the wrapper almost certainly exists already.
2. **`sysdefs` and `trident_api::constants`** for any constant you are about
   to type as a literal.
3. **Existing workspace dependencies** before adding a new one. Skim
   `[workspace.dependencies]` in the root `Cargo.toml` — `glob`, `regex`,
   `itertools`, `serde_yaml`, `tempfile`, `humantime`, `chrono`, `uuid`,
   `which`, `tera`, `tar`, `zstd`, `oci-client`, etc. are already in the
   tree. Adding a new top-level dep should be a deliberate decision, not a
   reflex.
4. **If the right home is `osutils` but the wrapper doesn't exist**, add
   the wrapper in `osutils` and call it from the subsystem. **Binary
   invocations belong in `osutils`** — do not inline a
   `std::process::Command::new("…")` invocation inside a subsystem or
   anywhere outside `osutils` (one-off exceptions exist, but each is a
   reviewable choice, not a default).
5. **Inside `osutils`, route binary invocations through the
   `osutils::dependencies::Dependency` enum**, not raw
   `std::process::Command`. Use
   `osutils::dependencies::Dependency::Foo.cmd().arg(…).run_and_check()` so
   the binary is `which`-resolved, errors are uniformly typed
   (`DependencyError` → `TridentError` with the right
   `MissingBinary`/`CommandCouldNotExecute`/`CommandFailed` variants), and
   the dependency appears in the workspace's central registry. If the
   binary you need isn't in the `Dependency` enum yet, **add it to the
   enum** — don't bypass the enum.
6. **Generic OS-concept path manipulation belongs in `osutils`** — anything
   that converts a system identifier into a kernel-blessed path (e.g.
   partition UUID → `/dev/disk/by-partuuid/<uuid>`, label →
   `/dev/disk/by-label/<…>`, mount point munging, `/sys`/`/proc` lookups).
   See `osutils::block_devices`, `osutils::path`,
   `osutils::mountpoint`, etc. for existing helpers — extend them rather
   than reconstructing the path inline in a subsystem.

### When in doubt

Prefer **adding a small wrapper to the right layer and calling it** over
duplicating a system invocation in a subsystem. The two most common
review nits in this category are: (a) "this `std::process::Command::new("foo")`
belongs in `osutils::foo`, routed through `Dependency::Foo`"; (b) "this
constant duplicates `trident_api::constants::FOO`".

## Reviewer etiquette (Nits & Architecture)

The "What to avoid" rules at the top of this file still apply: **do not open
a separate review comment just to flag a nit on otherwise-correct code.**
Nits and architectural notes are for writers to follow proactively, and for
reviewers to use as a checklist when a diff already touches the area.
Specifically:

- **Don't drag pre-existing violations into a PR's diff.** If a file already
  violates a nit (or already has a layering issue) on untouched lines,
  ignore it. Use a separate, dedicated PR.
- **Cluster comments.** When a diff has several small nits in one region,
  leave **one** comment listing them — not one per occurrence.
- **Never block a PR on a nit alone.** Mark nit-only comments as
  non-blocking, or fold them into a broader comment whose primary point is
  substantive.
- **Layering violations are blocking when they introduce a _new_ dependency
  across layers** (e.g., a new `std::process::Command::new(...)` outside
  `osutils`, or inside `osutils` bypassing the `Dependency` enum; a new
  `osutils` function that takes a `HostConfiguration` and decides what to
  do; a new `trident_api` item that performs I/O). They are not blocking
  when they only continue an existing local pattern.
