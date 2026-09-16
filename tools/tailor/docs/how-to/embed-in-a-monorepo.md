# Embed tailor in a monorepo

This guide sets tailor up to live as a **subdirectory of a larger repository**
(for example `<monorepo>/tailor/`) while keeping its own Cargo workspace,
version line, and release process. It also covers keeping a separate repository
as an experimentation fork.

The model here is **Option A: embedded in place, separate Cargo structures** —
tailor stays a self-contained workspace, and releases are cut from
component-scoped tags so they never collide with the host repository's own tags.

## 1. Keep tailor a self-contained Cargo workspace

tailor has its own `[workspace]` root (`tailor/Cargo.toml`) and its own
`Cargo.lock`. A repository can only have one workspace root per directory tree,
so if the monorepo has its **own** root `Cargo.toml` workspace, exclude tailor
from it:

```toml
# <monorepo>/Cargo.toml — the monorepo's own workspace
[workspace]
members = ["..."]
exclude = ["tailor"]   # tailor resolves as its own nested workspace
```

If the monorepo has no root workspace, nothing is needed — tailor's workspace is
discovered on its own. Either way tailor keeps:

- its independent `[workspace.package] version` (e.g. `1.0.0`),
- its own `Cargo.lock`,
- `rust-toolchain.toml`, `rustfmt.toml`, and lints.

Build and test tailor with its manifest, e.g. from the monorepo root:

```bash
cargo test  --manifest-path tailor/Cargo.toml --workspace --locked
cargo build --manifest-path tailor/Cargo.toml --release -p tailor
```

## 2. Relocate the release workflow

GitHub only runs workflows from the repository root `.github/workflows/`. When
embedding, move `tailor/.github/workflows/release.yml` to the monorepo's
`.github/workflows/` (rename it, e.g. `tailor-release.yml`) and make three
edits:

1. **Point it at the subdirectory.** The workflow already reads a single knob:

   ```yaml
   env:
     TAILOR_DIR: "tailor"   # was "." when tailor is the repo root
   ```

   Every cargo step runs in `TAILOR_DIR`, the version gate reads
   `${TAILOR_DIR}/Cargo.toml`, and the SBOM scans `${TAILOR_DIR}` — so this is
   the only value that changes.

2. **Scope the trigger to the subtree** so unrelated monorepo pushes do not fire
   it:

   ```yaml
   on:
     push:
       tags: ["tailor-v*"]
   ```

   (The tag filter already isolates tailor; add a `paths:` filter to any
   *branch*-triggered jobs you add.)

3. **Keep the component-scoped tag scheme** (below).

Do the same for CI if you run tailor's CI separately: set `TAILOR_DIR`, add
`paths: ['tailor/**']` to the `push`/`pull_request` triggers, and run cargo with
`--manifest-path tailor/Cargo.toml` (or a `working-directory`). Many monorepos
instead fold component CI into their unified pipeline — either is fine.

## 3. Release with component-scoped tags

Releases are triggered by a **`tailor-v<version>`** tag (not a bare `v<version>`,
which belongs to the host repository). The release gate strips the `tailor-`
prefix and requires the tag to match `tailor/Cargo.toml`'s
`[workspace.package] version`.

To cut a release:

```bash
# 1. Bump the version in tailor/Cargo.toml ([workspace.package] version) and
#    update tailor/CHANGELOG.md, then commit.
# 2. Tag and push:
git tag -a tailor-v1.1.0 -m "tailor 1.1.0"
git push origin tailor-v1.1.0
```

The workflow runs the gates (version==tag, tests, clippy, fmt), builds the musl
binaries, signs them with cosign keyless, generates a CycloneDX SBOM, attaches a
build-provenance attestation, and publishes a GitHub Release for the tag.

> If the host organization requires releases to go through an internal pipeline
> or package feed rather than GitHub Releases, replace the `release` job's
> publish/sign steps with that pipeline. Keep the version/test gates and the
> SBOM; swap cosign for the internal signer.

## 4. Update provenance verification

Keyless signatures and attestations are bound to the **repository and workflow**
that produced them. After moving, the verification identity changes from the
fork's repo to the host repo. Update the `repo` and the identity regexp in the
[verification instructions](../../README.md#verifying-releases):

```
identity="https://github.com/<host-org>/<host-repo>/.github/workflows/tailor-release.yml@refs/tags/tailor-v.*"
```

Artifacts released before the move remain verifiable against the old identity;
document both during the transition.

## 5. Documentation website

The docs site (MkDocs Material + `mike`, published to GitHub Pages) is **more
tied to a repository than the code is**, because GitHub Pages is one site per
repository: `mike` deploys to that repo's `gh-pages` branch, and the site URL is
`https://<owner>.github.io/<repo>/`. A host monorepo usually already owns its
Pages, so tailor's versioned docs cannot simply move into it. Three options, in
order of preference:

1. **Host the docs on this fork (recommended).** Keep the `docs.yml` workflow and
   the Pages site on the fork (`https://<you>.github.io/tailor/`). The monorepo
   holds the source-of-truth Markdown under `tailor/docs/`; sync it to the fork,
   which renders and publishes the site. Zero new infrastructure, and the
   existing versioned-docs setup keeps working. To publish a release's docs
   version, tag the fork `tailor-v<x.y.z>` (or run the docs workflow manually
   from the synced commit).
2. **Dedicated docs repo / org Pages.** Publish to a separate `tailor-docs` (or
   `<org>.github.io`) repository. The docs workflow builds the site and pushes it
   there with a deploy key or cross-repo Pages deploy. More setup, but keeps docs
   independent of both the fork and the monorepo.
3. **Sub-path of the monorepo's Pages.** If the monorepo publishes its own Pages,
   integrate tailor's docs as a subdirectory of that site. This means adopting the
   monorepo's docs pipeline (its generator, its versioning) — `mike`'s
   per-project versioning usually will not fit, so treat this as a rewrite.

Whichever you pick, the docs workflow already uses the component-scoped
`tailor-v*` tag (not `v*`), and `mkdocs.yml`'s `site_url`, `repo_url`, and
`edit_uri` point at whichever repo hosts the site — update them if you move it.

## 6. Keep a fork for experimentation
You can keep your original repository as an upstream experimentation fork:

- **One canonical release origin.** Cut official `tailor-v<x.y.z>` releases from
  the **host monorepo**. From the fork, cut only **prerelease** tags that can
  never collide, e.g. `tailor-v1.2.0-exp.1` (mark them as pre-releases). The
  same workflow handles both; only the identity differs.
- **Sync direction.** Move changes fork → monorepo with `git subtree`
  (`git subtree pull --prefix=tailor <fork-remote> main`) or ordinary PRs that
  copy the subtree. Pick one direction of truth for each branch to avoid
  divergence.
- **Versioning.** Never publish the same `tailor-v<x.y.z>` from two origins.
  Reserve release versions for the monorepo; use `-exp.N`/`-rc.N` suffixes in
  the fork.
