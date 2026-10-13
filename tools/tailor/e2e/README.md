# tailor end-to-end fixture

A small, real workspace the **E2E workflow** (`.github/workflows/tailor.yml`) builds with the actual
`tailor` binary and Image Customizer — to prove the full product works, end to end. The images are
built but **not booted**.

It is intentionally a real (not synthetic) build, so it exercises the slow/privileged path the unit
and integration tests can't: pulling the IC image and an Azure Linux base from MCR and running a
privileged `customize` that produces real `.cosi` artifacts.

## What it covers

One image, `appliance`, built into two cells, touching a broad slice of tailor in a single build:

- an **`azureLinux` (MCR) base** — digest-pinned by tailor and downloaded by IC (this needs the
  `input-image-oci` preview feature, declared in the image's `config:`; tailor is config-opaque so
  the opt-in lives there);
- a **matrix axis** (`flavor: [min, net]`) → two cells / two artifacts;
- **per-axis fragments** (`by-flavor/`) that set a parameter and, for `net`, append a kernel
  command-line argument via list merge;
- **parameter interpolation** (`${hostname}`) into the IC config;
- **`$include`** to splice a shared storage layout into every cell;
- the built-in **defaults** for `runtime.imageCacheDir` and `runtime.janitorImage` (the workspace
  sets neither), and the **sudo-free janitor** (the workflow asserts the outputs are runner-owned,
  not root-owned).

The workflow also runs the pure verbs (`list`, `matrix`, `validate`, `explain`, `render`),
which exercise the config/render layer without an engine.

## Run it locally

Requires Docker (the build runs a privileged IC container):

```bash
cd e2e
tailor build          # builds artifacts/appliance_{min,net}_amd64_cosi.cosi
```

## Extend it

- **Add a cell**: add a value to the `flavor` axis (and a `by-flavor/<value>.yaml` fragment), or add
  another matrix axis. Each cell is one real IC build, so keep the count modest to bound CI time.
- **Add an image**: drop in another `<name>/image.yaml`. The workflow's assertion loop lists the
  expected artifact slugs — add the new ones there.
- **Exercise more IC features**: extend the `config:` (e.g. packages, scripts, users). Package
  installs reach the network inside the build, so prefer them only when worth the extra time.

## SELinux janitor regression (Fedora VM only)

`selinux_janitor_e2e.py` exercises real rootful Podman containers and the actual Tailor `clean`
command, **not** a mocked runtime. This is a manual harness, **not wired into CI**: there is no
dedicated SELinux-enforcing runner. Run it as a non-root user *inside* a disposable Fedora VM with
`getenforce` reporting `Enforcing`. The user needs access to a rootful Podman API socket; granting
that access is effectively granting root on the VM, so **never** use a shared or production runner.
The script aborts unless the daemon reports SELinux enabled and rootless false. It uses only a
unique child of `~/tailor-selinux-e2e-work/` in the guest, requires the bind source to be
`user_home_t`, and never binds `/`, changes SELinux policy, relabels any host directory, or runs
host-side `sudo`. Keep this fixture out of the normal Ubuntu-hosted Tailor job: Ubuntu's Docker
runner cannot prove an enforcing Fedora SELinux denial.

Build this checkout's `tailor` binary and copy it beside the script in the VM. With the guest
Podman socket at `/run/podman/podman.sock`, run:

```text
python3 selinux_janitor_e2e.py --tailor-bin ./tailor
```

This **diagnostic** mode positively demonstrates that an unmodified confined janitor (`container_t`)
is denied access to a root-owned `user_home_t` artifact, that the current non-root `tailor clean`
also fails, and that a container-scoped `label=disable` janitor removes the *same* artifact without
changing labels or policy. That option is a measured workaround, **not** a production fix: disabling
the janitor container's SELinux confinement has security implications that require review. A genuine
fix needs a separate design decision. Automatic `:z`/`:Z` is **not** a safe fallback: janitor
removal binds the *parent* so `rm` can delete the child, but a parent can be an image directory or
RPM source containing unrelated user files. Confined `rm` needs write access to that parent;
relabeling its bind source would also relabel those unrelated files. The tested per-container
`label=disable` approach does not change host labels or disable host SELinux, but it weakens
isolation for that janitor container. This test does not claim to run a full Image Customizer build.

To compare a proposed fix **on the same running VM**, copy both binaries into the guest and run
the script twice: first `--tailor-bin ./tailor-baseline`, then
`--tailor-bin ./tailor-patched --expect-fixed`. Each invocation creates a uniquely
named disposable workspace, so the baseline cannot remove the patched test's fixture. The second
run is a **gating** check: the unmodified bind must still fail as a positive control, then
**Tailor itself** must remove the root-owned artifact as a non-root caller. Put that job on a
dedicated, ephemeral SELinux-enforcing Fedora VM runner (for example, a self-hosted runner labeled
`fedora-selinux-e2e`), not in a container on an Ubuntu runner. Provision Fedora 43 Cloud Base,
a 25 GiB guest disk, 4 GiB RAM, KVM, Podman, and a non-root runner with access to the rootful Podman
socket; assert
`getenforce=Enforcing` before running. Build Tailor from `tools/tailor/` with its pinned Rust
toolchain and public crates.io, then run this Python script inside the VM. Until Tailor's janitor
is fixed, `--expect-fixed` is expected to fail and must not be represented as a passing test.

Before each fixture deletion, the harness reads the guest's `/sys/block/loop*/loop/backing_file`
(including entries ending in ` (deleted)`) and `/proc/self/mountinfo` (mount root, mountpoint,
and source). A reference under the exact target path or an unreadable/malformed input fails
closed; it never detaches a loop or unmounts anything. Run
`python3 test_selinux_janitor_liveness.py` for synthetic path, mountinfo-escape, and file-backed
scanner checks that create **no live devices or mounts**. These fixture-side checks only protect
the named disposable artifact: they do **not** prove Tailor will reject a live build-scratch
tree or close races/other mount namespaces. In particular, a client-side scan is not
authoritative for a remote container daemon: its loops and mounts may be invisible here.
Do not infer liveness from artifact timestamps, substitute this preflight for a daemon-side
safety contract, or use an unguarded host-root deletion fallback. The tested minimal
janitor-only SELinux prototype has no production liveness guard: do not place real live loops
or mounts under a deletable fixture to try to validate one.

### Bounded pre-IC stale tools-dir probe

The default harness runs `tailor clean base-os` on a disposable root-owned artifact. To check
the **distinct pre-IC build path**, run it twice on the same Fedora VM, adding
`--probe-stale-tools-dir` to both invocations and `--expect-fixed` to the patched one. It creates
a root-owned `user_home_t` stale `<buildDirBase>/<cell>/tools-dir` only in the guest workspace
and invokes actual non-root `tailor build preic`. Both the named tools-dir source and the
intentionally *fake* IC toolchain use the small `base/core:3.0` image. Baseline fails at the
janitor's stale-copy removal with Permission denied; the SELinux janitor prototype removes the
stale marker and reaches the deliberately absent `customize` command. Both CLI builds exit
non-zero, for **different asserted reasons**. No image is built and post-build chown is not
exercised; do not report this as a successful IC build. Before running, `build --dry-run` must
show IC bind sources only inside that disposable guest workspace; the fixture disables the
otherwise unnecessary `/dev` bind. It never binds the guest or Ubuntu filesystem root.
For a future enforcing-Fedora CI gate, run both:

```text
python3 selinux_janitor_e2e.py --tailor-bin ./tailor --expect-fixed
python3 selinux_janitor_e2e.py --tailor-bin ./tailor --probe-stale-tools-dir --expect-fixed
```

Each must exit zero, although the fake-IC `tailor build` subprocess deliberately exits
non-zero. This needs access to MCR's small `base/core:3.0` image but no Image Customizer
image or real base-image download.

### What a full build regression would require

An additional, separate guest-only test could use one real Azure Linux `minimal-os` input, a
single raw output, `toolsDir: {source: base}` with the Azure Linux `base/core:3.0` source, and
IC's `input-image-oci` and `tools-dir` preview features. Pre-create a root-owned stale
`<buildDirBase>/<cell>/tools-dir` **only inside that unique disposable guest workspace**; assert
the guest has no live loop backing file or mount under that target before invoking non-root
`tailor build`. After a real successful IC run, verify the stale copy was replaced/reclaimed,
the resulting artifact is owned by the caller, and guest labels are unchanged. Do not detach
loops, unmount, or recursively clean up a failed build: stop and discard the disposable VM.
Use a larger dedicated runner than the measured *clean*-only VM (estimate at least 8 GiB RAM
and 40 GiB guest disk, plus outbound MCR access and a longer job timeout); these full-build
resources and the real IC path have **not** been measured by this harness.
